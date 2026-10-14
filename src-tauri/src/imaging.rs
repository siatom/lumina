//! Core image pipeline: format detection, metadata, EXIF, decoding,
//! thumbnailing and format conversion.
//!
//! This module is deliberately UI-agnostic (no Tauri types) so it can be unit
//! tested in isolation and reused by the IPC commands and the custom protocol.

use std::fs;
use std::io::{BufReader, Cursor};
use std::path::Path;

use image::{DynamicImage, ImageFormat, ImageReader};
use serde::Serialize;

use crate::error::AppError;

/// File extensions Lumina can open (lower-case, without the dot).
pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff", "ico",
];

/// Formats a webview can render directly, so the original file bytes can be
/// streamed without a re-encode. TIFF is intentionally excluded (browsers do
/// not render it) and is transcoded to PNG by [`read_for_display`].
pub const WEBVIEW_NATIVE_EXTENSIONS: &[&str] =
    &["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico"];

/// Basic EXIF payload surfaced to the UI.
#[derive(Debug, Clone, Serialize, Default)]
pub struct ExifData {
    pub orientation: Option<u16>,
    pub make: Option<String>,
    pub model: Option<String>,
    pub datetime: Option<String>,
    pub software: Option<String>,
    pub exposure_time: Option<String>,
    pub f_number: Option<String>,
    pub iso: Option<String>,
    pub focal_length: Option<String>,
}

/// Everything the frontend needs to render the info panel for one image.
#[derive(Debug, Clone, Serialize)]
pub struct ImageMetadata {
    pub path: String,
    pub file_name: String,
    /// Normalised format name, e.g. "png", "jpeg", "tiff".
    pub format: String,
    pub width: u32,
    pub height: u32,
    pub file_size: u64,
    pub has_exif: bool,
    pub exif: Option<ExifData>,
}

/// One image file inside a directory listing.
#[derive(Debug, Clone, Serialize)]
pub struct ImageEntry {
    pub path: String,
    pub file_name: String,
    pub format: String,
    pub file_size: u64,
}

/// Result of [`list_directory`]: the sibling images plus the index of the
/// currently opened file (if it is inside `dir`).
#[derive(Debug, Clone, Serialize)]
pub struct DirectoryListing {
    pub dir: String,
    pub entries: Vec<ImageEntry>,
    pub index: Option<usize>,
}

// ---------------------------------------------------------------------------
// Path / format helpers
// ---------------------------------------------------------------------------

/// Lower-case extension of `path` ("" when there is none).
pub fn extension_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Whether `path` has an extension Lumina supports.
pub fn is_supported(path: &Path) -> bool {
    SUPPORTED_EXTENSIONS.contains(&extension_of(path).as_str())
}

/// MIME type for a lower-case extension.
pub fn mime_for_ext(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "ico" => "image/x-icon",
        _ => "application/octet-stream",
    }
}

/// Normalised, human-readable name for an [`ImageFormat`].
///
/// Uses `extensions_str()` so it stays correct across `image` crate versions
/// that add new format variants.
pub fn format_name(format: ImageFormat) -> String {
    match format.extensions_str().first() {
        Some(ext) => (*ext).to_string(),
        None => "unknown".to_string(),
    }
}

/// Build a decoder that guesses the format from the file content (with an
/// extension fallback) instead of trusting the extension blindly.
fn reader_for(path: &Path) -> Result<ImageReader<BufReader<fs::File>>, AppError> {
    let reader = ImageReader::open(path)?.with_guessed_format()?;
    Ok(reader)
}

// ---------------------------------------------------------------------------
// Metadata
// ---------------------------------------------------------------------------

/// Read dimensions + format from the header only (no full decode).
pub fn read_metadata(path: &Path) -> Result<ImageMetadata, AppError> {
    if !path.is_file() {
        return Err(AppError::Other(format!(
            "not a readable file: {}",
            path.display()
        )));
    }

    let file_size = fs::metadata(path)?.len();
    let reader = reader_for(path)?;

    let format = reader
        .format()
        .map(format_name)
        .unwrap_or_else(|| extension_of(path));

    // `into_dimensions` parses just the image header.
    let (width, height) = reader.into_dimensions()?;

    let exif = read_exif(path);
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();

    Ok(ImageMetadata {
        path: path.to_string_lossy().into_owned(),
        file_name,
        format,
        width,
        height,
        file_size,
        has_exif: exif.is_some(),
        exif,
    })
}

/// Extract a small, curated subset of EXIF tags.
///
/// Returns `None` when the file has no EXIF block or it cannot be parsed — a
/// missing EXIF block is normal (PNG/BMP frequently have none), not an error.
fn read_exif(path: &Path) -> Option<ExifData> {
    let file = fs::File::open(path).ok()?;
    let mut buf = BufReader::new(file);
    let exif = exif::Reader::new().read_from_container(&mut buf).ok()?;

    // Match by tag name so we do not depend on exact enum variants across
    // `kamadak-exif` versions.
    let value_of = |name: &str| -> Option<String> {
        exif.fields()
            .find(|f| f.tag.to_string() == name)
            .map(|f| f.display_value().to_string())
    };

    let orientation = exif
        .get_field(exif::Tag::Orientation, exif::In::PRIMARY)
        .and_then(|f| f.value.get_uint(0))
        .map(|v| v as u16);

    Some(ExifData {
        orientation,
        make: value_of("Make"),
        model: value_of("Model"),
        datetime: value_of("DateTimeOriginal").or_else(|| value_of("DateTime")),
        software: value_of("Software"),
        exposure_time: value_of("ExposureTime"),
        f_number: value_of("FNumber"),
        iso: value_of("PhotographicSensitivity").or_else(|| value_of("ISOSpeedRatings")),
        focal_length: value_of("FocalLength"),
    })
}

// ---------------------------------------------------------------------------
// Decode / encode
// ---------------------------------------------------------------------------

/// Fully decode an image into memory.
pub fn decode(path: &Path) -> Result<DynamicImage, AppError> {
    Ok(reader_for(path)?.decode()?)
}

/// Encode a decoded image as PNG bytes.
pub fn encode_png(img: &DynamicImage) -> Result<Vec<u8>, AppError> {
    let mut buf = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)?;
    Ok(buf)
}

/// Produce a small PNG thumbnail that fits inside `max_size` on both axes,
/// preserving aspect ratio. Always PNG so the result is webview-safe.
pub fn make_thumbnail(path: &Path, max_size: u32) -> Result<Vec<u8>, AppError> {
    let img = decode(path)?;
    let thumb = img.thumbnail(max_size, max_size);
    encode_png(&thumb)
}

/// Bytes to hand to an `<img>` element for the *original* view.
///
/// For webview-native formats we stream the original file untouched (no
/// re-encode, best quality + speed for large photos). For anything else
/// (currently TIFF) we decode and transcode to PNG.
pub fn read_for_display(path: &Path) -> Result<(Vec<u8>, &'static str), AppError> {
    let ext = extension_of(path);
    if WEBVIEW_NATIVE_EXTENSIONS.contains(&ext.as_str()) {
        let bytes = fs::read(path)?;
        Ok((bytes, mime_for_ext(&ext)))
    } else {
        let img = decode(path)?;
        Ok((encode_png(&img)?, "image/png"))
    }
}

/// Re-encode `input` into `output`. When `format` is `None` the output format
/// is inferred from the output file extension.
pub fn convert_image(
    input: &Path,
    output: &Path,
    format: Option<ImageFormat>,
) -> Result<(), AppError> {
    let img = decode(input)?;
    match format {
        Some(f) => img.save_with_format(output, f)?,
        None => img.save(output)?,
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Directory listing
// ---------------------------------------------------------------------------

/// List sibling images in `dir`, naturally sorted by file name, and report the
/// index of `current` when it belongs to the listing.
pub fn list_directory(dir: &Path, current: Option<&Path>) -> Result<DirectoryListing, AppError> {
    if !dir.is_dir() {
        return Err(AppError::Other(format!(
            "not a directory: {}",
            dir.display()
        )));
    }

    let mut entries: Vec<ImageEntry> = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() || !is_supported(&path) {
            continue;
        }
        let file_size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        entries.push(ImageEntry {
            path: path.to_string_lossy().into_owned(),
            file_name,
            format: extension_of(&path),
            file_size,
        });
    }

    entries.sort_by(|a, b| natural_cmp(&a.file_name, &b.file_name));

    let index = current.and_then(|c| {
        let c = c.to_string_lossy();
        entries.iter().position(|e| e.path == c)
    });

    Ok(DirectoryListing {
        dir: dir.to_string_lossy().into_owned(),
        entries,
        index,
    })
}

/// Case-insensitive natural ordering: `img2` sorts before `img10`.
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();

    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    let mut na = String::new();
                    while matches!(ai.peek(), Some(c) if c.is_ascii_digit()) {
                        na.push(ai.next().unwrap());
                    }
                    let mut nb = String::new();
                    while matches!(bi.peek(), Some(c) if c.is_ascii_digit()) {
                        nb.push(bi.next().unwrap());
                    }
                    let va: u128 = na.parse().unwrap_or(0);
                    let vb: u128 = nb.parse().unwrap_or(0);
                    match va.cmp(&vb) {
                        Ordering::Equal => continue,
                        ord => return ord,
                    }
                } else {
                    let la = ca.to_ascii_lowercase();
                    let lb = cb.to_ascii_lowercase();
                    match la.cmp(&lb) {
                        Ordering::Equal => {
                            ai.next();
                            bi.next();
                        }
                        ord => return ord,
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_supported_extensions() {
        assert!(is_supported(Path::new("a.PNG")));
        assert!(is_supported(Path::new("dir/b.jpeg")));
        assert!(!is_supported(Path::new("a.txt")));
        assert!(!is_supported(Path::new("noext")));
    }

    #[test]
    fn natural_order_is_numeric_aware() {
        let mut v = vec!["img10.png", "img2.png", "img1.png"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["img1.png", "img2.png", "img10.png"]);
    }

    #[test]
    fn mime_lookup() {
        assert_eq!(mime_for_ext("jpg"), "image/jpeg");
        assert_eq!(mime_for_ext("tiff"), "image/tiff");
        assert_eq!(mime_for_ext("bin"), "application/octet-stream");
    }

    /// End-to-end exercise of the pipeline on real files on disk:
    /// decode → metadata → thumbnail → display bytes → directory listing.
    #[test]
    fn pipeline_roundtrip() {
        let dir = std::env::temp_dir().join(format!("lumina-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        // 64x32 gradient.
        let img = image::RgbImage::from_fn(64, 32, |x, y| {
            image::Rgb([(x * 4) as u8, (y * 8) as u8, 128])
        });
        let png = dir.join("sample.png");
        img.save(&png).unwrap();

        // --- metadata (header-only) ---
        let meta = read_metadata(&png).unwrap();
        assert_eq!((meta.width, meta.height), (64, 32));
        assert_eq!(meta.format, "png");
        assert_eq!(meta.file_name, "sample.png");
        assert!(meta.file_size > 0);

        // --- thumbnail: fits the box, keeps aspect ratio, is a real PNG ---
        let thumb = make_thumbnail(&png, 16).unwrap();
        let decoded = image::load_from_memory(&thumb).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (16, 8));

        // --- display bytes for a webview-native format are the original bytes ---
        let (bytes, mime) = read_for_display(&png).unwrap();
        assert_eq!(mime, "image/png");
        assert_eq!(bytes.len() as u64, meta.file_size);

        // --- TIFF is not webview-native, so it must be transcoded to PNG ---
        let tiff = dir.join("sample.tiff");
        img.save_with_format(&tiff, ImageFormat::Tiff).unwrap();
        assert_eq!(read_metadata(&tiff).unwrap().format, "tiff");
        let (tiff_bytes, tiff_mime) = read_for_display(&tiff).unwrap();
        assert_eq!(tiff_mime, "image/png");
        assert_eq!(&tiff_bytes[0..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);

        // --- listing: natural order + current index ---
        std::fs::write(dir.join("sample2.png"), &bytes).unwrap();
        std::fs::write(dir.join("sample10.png"), &bytes).unwrap();
        let listing = list_directory(&dir, Some(&png)).unwrap();
        let names: Vec<_> = listing.entries.iter().map(|e| e.file_name.clone()).collect();
        assert_eq!(
            names,
            vec!["sample.png", "sample.tiff", "sample2.png", "sample10.png"]
        );
        assert_eq!(listing.index, Some(0));

        std::fs::remove_dir_all(&dir).ok();
    }
}
