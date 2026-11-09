//! Core image pipeline: format detection, metadata, EXIF, decoding,
//! thumbnailing, format conversion and image editing operations.
//!
//! This module is deliberately UI-agnostic (no Tauri types) so it can be unit
//! tested in isolation and reused by the IPC commands and the custom protocol.

use std::fs;
use std::io::{BufReader, Cursor};
use std::path::{Path, PathBuf};
use std::collections::VecDeque;
use std::sync::{mpsc, Arc, Mutex, OnceLock};

use image::codecs::jpeg::JpegEncoder;
use image::codecs::webp::WebPEncoder;
use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{AnimationDecoder, DynamicImage, ExtendedColorType, ImageEncoder, ImageFormat, ImageReader};
use serde::{Deserialize, Serialize};

use resvg::{tiny_skia, usvg};

use crate::error::AppError;

/// File extensions Lumina can open (lower-case, without the dot).
pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff", "ico", "svg",
];

/// Formats a webview can render directly, so the original file bytes can be
/// streamed without a re-encode. TIFF is intentionally excluded (browsers do
/// not render it) and is transcoded to PNG by [`read_for_display`]. SVG *is*
/// rendered natively by the webview, so it is streamed untouched as well.
///
/// # EXIF orientation (important)
///
/// For these formats [`read_for_display`] returns the **original bytes,
/// orientation tag included, and must not rotate anything**. Blink (WebView2 /
/// Edge, verified on 155.0.4283.45) applies EXIF orientation itself, following
/// the CSS `image-orientation: from-image` default; the rotation is baked into
/// the decoded bitmap, so it also affects `naturalWidth`/`naturalHeight` and
/// canvas `drawImage`. Applying the rotation here as well would double-rotate.
///
/// Formats *outside* this list (TIFF) and every Rust-side raster path
/// (thumbnails, frames, save-as, edits) produce output that the webview will
/// **not** auto-rotate, so they bake the orientation into the pixels instead —
/// see [`apply_exif_orientation`].
pub const WEBVIEW_NATIVE_EXTENSIONS: &[&str] =
    &["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "svg"];

/// Longest-edge cap when rasterising an SVG for metadata / thumbnails, so a
/// pathological `width="100000"` document cannot allocate gigabytes.
const SVG_MAX_DECODE_EDGE: u32 = 4096;

/// Full-resolution thumbnail decodes are serialized across formats that do
/// not support decode-time scaling.
static THUMB_DECODE_SLOT: OnceLock<Mutex<()>> = OnceLock::new();

/// Default JPEG quality when the caller does not supply one.
pub const DEFAULT_JPEG_QUALITY: u8 = 85;

/// Basic EXIF payload surfaced to the UI.
#[derive(Debug, Clone, Serialize, Default)]
pub struct ExifData {
    pub orientation: Option<u16>,
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens_model: Option<String>,
    pub datetime: Option<String>,
    pub software: Option<String>,
    pub artist: Option<String>,
    pub copyright: Option<String>,
    pub description: Option<String>,
    pub exposure_time: Option<String>,
    pub f_number: Option<String>,
    pub iso: Option<String>,
    pub focal_length: Option<String>,
    /// Decimal degrees, negative for southern hemisphere.
    pub gps_latitude: Option<f64>,
    /// Decimal degrees, negative for western hemisphere.
    pub gps_longitude: Option<f64>,
    /// Metres above sea level.
    pub gps_altitude: Option<f64>,
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
    /// Last-modified time in milliseconds since the Unix epoch (0 if unknown).
    /// Used by the frontend as a cache-busting version token.
    pub modified_ms: u64,
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
    /// Last-modified time in milliseconds since the Unix epoch (0 if unknown).
    pub modified_ms: u64,
}

/// Result of [`list_directory`]: the sibling images plus the index of the
/// currently opened file (if it is inside `dir`).
#[derive(Debug, Clone, Serialize)]
pub struct DirectoryListing {
    pub dir: String,
    pub entries: Vec<ImageEntry>,
    pub index: Option<usize>,
}

/// Rectangle in *source* pixel coordinates. Values are clamped to the image
/// bounds before cropping.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CropRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Resize request. `exact = true` forces exactly `width` x `height` (may
/// distort); otherwise the aspect ratio is preserved and the result fits
/// inside the given box.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ResizeSpec {
    pub width: Option<u32>,
    pub height: Option<u32>,
    #[serde(default)]
    pub exact: Option<bool>,
    /// "nearest" | "triangle" | "catmullrom" | "gaussian" | "lanczos3" (default).
    #[serde(default)]
    pub filter: Option<String>,
}

/// Declarative image edit applied by [`transform_image`] / [`apply_transform`].
///
/// Operations are applied in a fixed, documented order so the frontend can
/// preview them predictably: **crop → flip → rotate → resize → encode**.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TransformSpec {
    /// 0 | 90 | 180 | 270 (clockwise).
    #[serde(default)]
    pub rotate: Option<u16>,
    #[serde(default)]
    pub flip_horizontal: Option<bool>,
    #[serde(default)]
    pub flip_vertical: Option<bool>,
    #[serde(default)]
    pub crop: Option<CropRect>,
    #[serde(default)]
    pub resize: Option<ResizeSpec>,
    /// Target format name; when omitted it is inferred from the output path.
    #[serde(default)]
    pub format: Option<String>,
    /// 1..=100. Only honoured by the JPEG encoder (WebP/PNG are lossless).
    #[serde(default)]
    pub quality: Option<u8>,
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

/// Last-modified time of `md` in milliseconds since the Unix epoch, or 0 when
/// the platform / filesystem does not report one.
fn modified_ms_of(md: &fs::Metadata) -> u64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Cache-busting revision token for `path`: `<modified_ms>-<file_size>`.
///
/// Matches the token the frontend builds from [`ImageMetadata`] / [`ImageEntry`]
/// so protocol requests and directory listings agree on the same cache key.
/// Empty when the file cannot be stat'd.
pub fn revision_of(path: &Path) -> String {
    match fs::metadata(path) {
        Ok(md) => format!("{}-{}", modified_ms_of(&md), md.len()),
        Err(_) => String::new(),
    }
}

/// Whether `path` is an SVG (handled by the vector rasteriser).
pub fn is_svg(path: &Path) -> bool {
    extension_of(path) == "svg"
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
        "svg" => "image/svg+xml",
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

/// Map a format name / extension onto an [`ImageFormat`].
///
/// AVIF and HEIC/HEIF are rejected with an explanatory message: neither has a
/// pure-Rust decoder (see `docs/INTERFACES.md`, "格式支持矩阵").
pub fn parse_format(name: &str) -> Result<ImageFormat, AppError> {
    match name.trim_start_matches('.').to_ascii_lowercase().as_str() {
        "png" => Ok(ImageFormat::Png),
        "jpg" | "jpeg" => Ok(ImageFormat::Jpeg),
        "gif" => Ok(ImageFormat::Gif),
        "webp" => Ok(ImageFormat::WebP),
        "bmp" => Ok(ImageFormat::Bmp),
        "tif" | "tiff" => Ok(ImageFormat::Tiff),
        "ico" => Ok(ImageFormat::Ico),
        "avif" => Err(AppError::Unsupported(
            "avif: 解码需要原生 dav1d 构建，已评估并推迟（见 docs/INTERFACES.md）".into(),
        )),
        "heic" | "heif" => Err(AppError::Unsupported(
            "heic/heif: 解码需要原生 libheif，已评估并推迟（见 docs/INTERFACES.md）".into(),
        )),
        other => Err(AppError::Unsupported(other.to_string())),
    }
}

/// Resolve the output format from an explicit name, falling back to the
/// output path's extension.
fn resolve_output_format(
    format: Option<&str>,
    output: &Path,
) -> Result<ImageFormat, AppError> {
    match format {
        Some(name) => parse_format(name),
        None => {
            let ext = extension_of(output);
            if ext.is_empty() {
                return Err(AppError::Other(
                    "cannot infer output format: no explicit format and no file extension".into(),
                ));
            }
            parse_format(&ext)
        }
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
    let md = fs::metadata(path)?;
    if !md.is_file() {
        return Err(AppError::Other(format!(
            "not a readable file: {}",
            path.display()
        )));
    }

    let file_size = md.len();
    let modified_ms = modified_ms_of(&md);
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();

    // SVG has no raster header: parse the vector document instead.
    if is_svg(path) {
        let (width, height) = svg_dimensions(path)?;
        return Ok(ImageMetadata {
            path: path.to_string_lossy().into_owned(),
            file_name,
            format: "svg".to_string(),
            width,
            height,
            file_size,
            modified_ms,
            has_exif: false,
            exif: None,
        });
    }

    let reader = reader_for(path)?;

    let format = reader
        .format()
        .map(format_name)
        .unwrap_or_else(|| extension_of(path));

    // `into_dimensions` parses just the image header.
    let (width, height) = reader.into_dimensions()?;

    let exif = read_exif(path);

    Ok(ImageMetadata {
        path: path.to_string_lossy().into_owned(),
        file_name,
        format,
        width,
        height,
        file_size,
        modified_ms,
        has_exif: exif.is_some(),
        exif,
    })
}

/// Find a field by its numeric tag id, regardless of which IFD (and therefore
/// which tag *context*) it lives in.
///
/// Matching by number — rather than by `Tag` equality or tag name — keeps the
/// lookup working for non-conforming files that place Exif-IFD tags in IFD0,
/// and is stable across `kamadak-exif` versions.
fn field_by_num(exif: &exif::Exif, num: u16) -> Option<&exif::Field> {
    exif.fields().find(|f| f.tag.number() == num)
}

/// Read a tag as a string: ASCII values verbatim (no surrounding quotes, which
/// `display_value()` would add), other types via the formatted display value.
fn field_string(exif: &exif::Exif, tag: exif::Tag) -> Option<String> {
    let field = field_by_num(exif, tag.number())?;
    match &field.value {
        exif::Value::Ascii(v) => v.first().map(|bytes| {
            String::from_utf8_lossy(bytes)
                .trim_end_matches('\0')
                .to_string()
        }),
        _ => Some(field.display_value().to_string()),
    }
}

/// Numeric value of a RATIONAL tag (first component).
fn field_rational(exif: &exif::Exif, tag: exif::Tag) -> Option<f64> {
    match &field_by_num(exif, tag.number())?.value {
        exif::Value::Rational(v) if !v.is_empty() => Some(v[0].to_f64()),
        _ => None,
    }
}

/// Extract a curated subset of EXIF tags (camera, time, lens, GPS).
///
/// Returns `None` when the file has no EXIF block or it cannot be parsed — a
/// missing EXIF block is normal (PNG/BMP frequently have none), not an error.
fn read_exif(path: &Path) -> Option<ExifData> {
    let file = fs::File::open(path).ok()?;
    let mut buf = BufReader::new(file);
    let exif = exif::Reader::new().read_from_container(&mut buf).ok()?;

    let orientation = field_by_num(&exif, exif::Tag::Orientation.number())
        .and_then(|f| f.value.get_uint(0))
        .map(|v| v as u16);

    // --- GPS: degrees/minutes/seconds → signed decimal degrees ---
    let dms = |tag: exif::Tag| -> Option<(f64, f64, f64)> {
        match &field_by_num(&exif, tag.number())?.value {
            exif::Value::Rational(v) if v.len() >= 3 => {
                Some((v[0].to_f64(), v[1].to_f64(), v[2].to_f64()))
            }
            _ => None,
        }
    };
    let ascii_of = |tag: exif::Tag| -> Option<String> {
        match &field_by_num(&exif, tag.number())?.value {
            exif::Value::Ascii(v) => v
                .first()
                .map(|b| String::from_utf8_lossy(b).trim_end_matches('\0').to_string()),
            _ => None,
        }
    };
    let to_decimal = |(d, m, s): (f64, f64, f64)| d + m / 60.0 + s / 3600.0;

    let gps_latitude = dms(exif::Tag::GPSLatitude).map(|dms| {
        let v = to_decimal(dms);
        if ascii_of(exif::Tag::GPSLatitudeRef).as_deref() == Some("S") {
            -v
        } else {
            v
        }
    });
    let gps_longitude = dms(exif::Tag::GPSLongitude).map(|dms| {
        let v = to_decimal(dms);
        if ascii_of(exif::Tag::GPSLongitudeRef).as_deref() == Some("W") {
            -v
        } else {
            v
        }
    });
    let gps_altitude = match field_by_num(&exif, exif::Tag::GPSAltitude.number()) {
        Some(f) => match &f.value {
            exif::Value::Rational(v) if !v.is_empty() => Some(v[0].to_f64()),
            _ => None,
        },
        None => None,
    };

    Some(ExifData {
        orientation,
        make: field_string(&exif, exif::Tag::Make),
        model: field_string(&exif, exif::Tag::Model),
        lens_model: field_string(&exif, exif::Tag::LensModel),
        datetime: field_string(&exif, exif::Tag::DateTimeOriginal)
            .or_else(|| field_string(&exif, exif::Tag::DateTime)),
        software: field_string(&exif, exif::Tag::Software),
        artist: field_string(&exif, exif::Tag::Artist),
        copyright: field_string(&exif, exif::Tag::Copyright),
        description: field_string(&exif, exif::Tag::ImageDescription),
        exposure_time: field_rational(&exif, exif::Tag::ExposureTime).map(format_exposure),
        f_number: field_rational(&exif, exif::Tag::FNumber).map(|v| format!("f/{v:.1}")),
        iso: field_string(&exif, exif::Tag::PhotographicSensitivity),
        focal_length: field_rational(&exif, exif::Tag::FocalLength)
            .map(|v| format!("{v:.0} mm")),
        gps_latitude,
        gps_longitude,
        gps_altitude,
    })
}

/// Render an exposure time: fractions below a second (`1/250`), seconds above.
fn format_exposure(seconds: f64) -> String {
    if seconds <= 0.0 {
        return "0 s".to_string();
    }
    if seconds < 1.0 {
        format!("1/{:.0}", (1.0 / seconds).round())
    } else {
        format!("{seconds:.1} s")
    }
}

// ---------------------------------------------------------------------------
// EXIF orientation
// ---------------------------------------------------------------------------
//
// Why this matters: a camera stores portrait shots as landscape pixels plus an
// EXIF `Orientation` tag telling viewers how to rotate them. Blink applies that
// tag for the `<img>` original view (see [`WEBVIEW_NATIVE_EXTENSIONS`]), but a
// PNG we synthesise in Rust carries no EXIF, so nothing downstream would
// rotate it. Every Rust-side raster path therefore bakes the orientation into
// the pixels, which keeps thumbnails / frames / saved files aligned with the
// main view.
//
// Verified behaviour of the engine (Edge/WebView2 155.0.4283.45, run
// `scripts\probe-webview2-orientation.cmd`) on a 96x64 fixture whose *stored*
// top-left corner is red:
//
//   orientation 1 -> natural 96x64, red at top-left      (no transform)
//   orientation 6 -> natural 64x96, red at top-right     (rotate 90 CW)
//   orientation 8 -> natural 64x96, red at bottom-left   (rotate 270 CW)
//
// That is exactly `image::metadata::Orientation::from_exif` semantics, so
// `DynamicImage::apply_orientation` reproduces the webview pixel-for-pixel.

/// EXIF orientation of a file, or `None` when it is absent / invalid / 1.
///
/// Read through `kamadak-exif`, i.e. the *same* source as
/// [`ImageMetadata::exif`], so the reported orientation and the applied
/// transform can never disagree. Works for JPEG, TIFF, WebP and PNG (eXIf);
/// SVG and BMP have no EXIF and yield `None`.
fn exif_orientation(path: &Path) -> Option<Orientation> {
    let file = fs::File::open(path).ok()?;
    let mut buf = BufReader::new(file);
    let exif = exif::Reader::new().read_from_container(&mut buf).ok()?;
    let raw = field_by_num(&exif, exif::Tag::Orientation.number())?.value.get_uint(0)?;
    // `from_exif` returns None for 0 and 9+, so bogus tags are ignored.
    Orientation::from_exif(raw as u8)
}

/// Bake the EXIF orientation of `path` into `img`'s pixels.
///
/// Lossless: every variant is a 90-degree rotation and/or a mirror, so no
/// resampling happens and no detail is lost.
pub fn apply_exif_orientation(img: &mut DynamicImage, path: &Path) {
    if let Some(orientation) = exif_orientation(path) {
        img.apply_orientation(orientation);
    }
}

/// Decode `path` and bake its EXIF orientation into the pixels.
///
/// Use this for anything that leaves the process (thumbnails, frames,
/// converted / edited files). Use [`decode`] when you specifically want the
/// stored pixels, and never apply this on the
/// [`WEBVIEW_NATIVE_EXTENSIONS`] passthrough path.
pub fn decode_oriented(path: &Path) -> Result<DynamicImage, AppError> {
    let mut img = decode(path)?;
    apply_exif_orientation(&mut img, path);
    Ok(img)
}

// ---------------------------------------------------------------------------
// SVG (vector) rasterisation
// ---------------------------------------------------------------------------

fn svg_error(e: usvg::Error) -> AppError {
    AppError::Other(format!("SVG error: {e}"))
}

/// Shared, lazily-populated system font database so `<text>` in SVG renders.
/// Loading happens once per process and only when the first SVG is opened.
fn system_fontdb() -> &'static Arc<usvg::fontdb::Database> {
    static DB: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        db.load_system_fonts();
        Arc::new(db)
    })
}

fn svg_options() -> usvg::Options<'static> {
    let mut opt = usvg::Options::default();
    opt.fontdb = system_fontdb().clone();
    opt
}

fn svg_tree(path: &Path) -> Result<usvg::Tree, AppError> {
    let data = fs::read(path)?;
    usvg::Tree::from_data(&data, &svg_options()).map_err(svg_error)
}

/// Intrinsic size of an SVG in CSS pixels (resolved from `width`/`height` or
/// `viewBox`, honouring the 96 DPI default).
pub fn svg_dimensions(path: &Path) -> Result<(u32, u32), AppError> {
    let tree = svg_tree(path)?;
    let size = tree.size();
    Ok((size.width().round().max(1.0) as u32, size.height().round().max(1.0) as u32))
}

/// Rasterise an SVG to RGBA8. `max_edge` caps the longest side (defaults to
/// [`SVG_MAX_DECODE_EDGE`]); the result keeps the document's aspect ratio.
pub fn rasterize_svg(path: &Path, max_edge: Option<u32>) -> Result<DynamicImage, AppError> {
    let tree = svg_tree(path)?;
    let size = tree.size();
    let (w_pt, h_pt) = (size.width(), size.height());

    let cap = max_edge.unwrap_or(SVG_MAX_DECODE_EDGE).max(1) as f32;
    let longest = w_pt.max(h_pt);
    let scale = if longest > cap { cap / longest } else { 1.0 };

    let w = ((w_pt * scale).round() as u32).max(1);
    let h = ((h_pt * scale).round() as u32).max(1);

    let mut pixmap = tiny_skia::Pixmap::new(w, h)
        .ok_or_else(|| AppError::Other("failed to allocate SVG pixmap".into()))?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );

    // tiny-skia stores premultiplied alpha; undo it for image's straight RGBA.
    let mut buf = Vec::with_capacity((w * h * 4) as usize);
    for p in pixmap.pixels() {
        let c = p.demultiply();
        buf.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }

    let img = image::RgbaImage::from_raw(w, h, buf)
        .ok_or_else(|| AppError::Other("bad SVG raster buffer".into()))?;
    Ok(DynamicImage::ImageRgba8(img))
}

// ---------------------------------------------------------------------------
// Decode / encode
// ---------------------------------------------------------------------------

/// Fully decode an image into memory, **exactly as stored**.
///
/// SVG documents are rasterised first. The EXIF orientation is *not* applied —
/// this is the raw-pixel primitive; use [`decode_oriented`] when the result is
/// going to be shown or written anywhere.
pub fn decode(path: &Path) -> Result<DynamicImage, AppError> {
    if is_svg(path) {
        return rasterize_svg(path, None);
    }
    Ok(reader_for(path)?.decode()?)
}

/// Encode a decoded image as PNG bytes.
pub fn encode_png(img: &DynamicImage) -> Result<Vec<u8>, AppError> {
    let mut buf = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)?;
    Ok(buf)
}

/// Encode `img` in `format`, optionally with a quality hint.
///
/// Quality is only honoured by the JPEG encoder (`1..=100`). WebP and PNG are
/// lossless in the `image` crate, so the parameter is ignored for them.
pub fn encode_image(
    img: &DynamicImage,
    format: ImageFormat,
    quality: Option<u8>,
) -> Result<Vec<u8>, AppError> {
    let mut buf = Vec::new();
    match format {
        ImageFormat::Jpeg => {
            let rgb = img.to_rgb8();
            let q = quality.unwrap_or(DEFAULT_JPEG_QUALITY).clamp(1, 100);
            JpegEncoder::new_with_quality(Cursor::new(&mut buf), q).write_image(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                ExtendedColorType::Rgb8,
            )?;
        }
        ImageFormat::WebP => {
            let rgba = img.to_rgba8();
            WebPEncoder::new_lossless(Cursor::new(&mut buf)).write_image(
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
                ExtendedColorType::Rgba8,
            )?;
        }
        other => img.write_to(&mut Cursor::new(&mut buf), other)?,
    }
    Ok(buf)
}

/// Produce a small PNG thumbnail that fits inside `max_size` on both axes,
/// preserving aspect ratio. Always PNG so the result is webview-safe.
///
/// The EXIF orientation **is** applied, because the main view (`<img>` on the
/// original file) has it applied by the engine — without this the thumbnail
/// and the main view would disagree for portrait phone photos. The rotation is
/// applied *after* the downscale: it is a pure 90-degree multiple (no
/// resampling, no quality loss) so the order is free, and rotating the small
/// image is much cheaper than rotating the full-size one.
pub fn make_thumbnail(path: &Path, max_size: u32) -> Result<Vec<u8>, AppError> {
    // SVG is rasterised straight at the target size instead of at full size.
    if is_svg(path) {
        let img = rasterize_svg(path, Some(max_size))?;
        let thumb = img.thumbnail(max_size, max_size);
        return encode_png(&thumb);
    }
    let img = if matches!(extension_of(path).as_str(), "jpg" | "jpeg") {
        decode_jpeg_thumbnail(path, max_size)?
    } else {
        // Formats without decode-time scaling share one full-resolution decode
        // slot, preventing the frontend's thumbnail pool from multiplying the
        // peak bitmap memory by its request concurrency.
        let _slot = THUMB_DECODE_SLOT
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        decode(path)?
    };
    let mut thumb = img.thumbnail(max_size, max_size);
    apply_exif_orientation(&mut thumb, path);
    encode_png(&thumb)
}

/// Decode JPEG thumbnails at the nearest supported DCT scale before allocating
/// pixel buffers. The regular image decoder does not currently expose this
/// option, so this path is limited to thumbnail generation.
fn decode_jpeg_thumbnail(path: &Path, max_size: u32) -> Result<DynamicImage, AppError> {
    let mut decoder = jpeg_decoder::Decoder::new(BufReader::new(fs::File::open(path)?));
    decoder
        .read_info()
        .map_err(|error| AppError::Other(format!("JPEG decode error: {error}")))?;
    let target = max_size.clamp(1, u16::MAX as u32) as u16;
    decoder
        .scale(target, target)
        .map_err(|error| AppError::Other(format!("JPEG scale error: {error}")))?;
    let pixels = decoder
        .decode()
        .map_err(|error| AppError::Other(format!("JPEG decode error: {error}")))?;
    let info = decoder
        .info()
        .ok_or_else(|| AppError::Other("JPEG dimensions unavailable".into()))?;
    match info.pixel_format {
        jpeg_decoder::PixelFormat::L16 => {
            let samples: Vec<u16> = pixels
                .chunks_exact(2)
                .map(|sample| u16::from_ne_bytes([sample[0], sample[1]]))
                .collect();
            image::ImageBuffer::<image::Luma<u16>, _>::from_raw(
                u32::from(info.width),
                u32::from(info.height),
                samples,
            )
                .map(DynamicImage::ImageLuma16)
                .ok_or_else(|| AppError::Other("JPEG pixel buffer size mismatch".into()))
        }
        jpeg_decoder::PixelFormat::L8 => {
            image::GrayImage::from_raw(u32::from(info.width), u32::from(info.height), pixels)
                .map(DynamicImage::ImageLuma8)
                .ok_or_else(|| AppError::Other("JPEG pixel buffer size mismatch".into()))
        }
        jpeg_decoder::PixelFormat::RGB24 => {
            image::RgbImage::from_raw(u32::from(info.width), u32::from(info.height), pixels)
                .map(DynamicImage::ImageRgb8)
                .ok_or_else(|| AppError::Other("JPEG pixel buffer size mismatch".into()))
        }
        jpeg_decoder::PixelFormat::CMYK32 => {
            // Preserve uncommon CMYK JPEG support through the standard decoder.
            let _slot = THUMB_DECODE_SLOT
                .get_or_init(|| Mutex::new(()))
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            decode(path)
        }
    }
}

/// Bytes to hand to an `<img>` element for the *original* view.
///
/// For webview-native formats (incl. SVG) we stream the original file
/// untouched (no re-encode, best quality + speed for large photos) — including
/// any EXIF orientation tag, which the engine applies itself (see
/// [`WEBVIEW_NATIVE_EXTENSIONS`]). For anything else (currently TIFF) we decode,
/// **bake the orientation into the pixels** and transcode to PNG, since the
/// PNG we hand over carries no EXIF for the engine to act on.
pub fn read_for_display(path: &Path) -> Result<(Vec<u8>, &'static str), AppError> {
    let ext = extension_of(path);
    if WEBVIEW_NATIVE_EXTENSIONS.contains(&ext.as_str()) {
        let bytes = fs::read(path)?;
        Ok((bytes, mime_for_ext(&ext)))
    } else {
        let mut img = decode(path)?;
        apply_exif_orientation(&mut img, path);
        Ok((encode_png(&img)?, "image/png"))
    }
}

/// Re-encode `input` into `output`. When `format` is `None` the output format
/// is inferred from the output file extension.
///
/// The EXIF orientation is baked into the pixels but the rest of the EXIF block
/// is not copied, so the result must not keep a stale orientation tag — baking
/// it is what makes the converted file display the same way as the original.
pub fn convert_image(
    input: &Path,
    output: &Path,
    format: Option<ImageFormat>,
    quality: Option<u8>,
) -> Result<(), AppError> {
    let img = decode_oriented(input)?;
    let fmt = match format {
        Some(f) => f,
        None => resolve_output_format(None, output)?,
    };
    let bytes = encode_image(&img, fmt, quality)?;
    fs::write(output, bytes)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Image editing operations
// ---------------------------------------------------------------------------

/// Apply a [`TransformSpec`] to an already decoded image.
///
/// Order is fixed and documented: **crop → flip → rotate → resize**.
pub fn apply_transform(
    mut img: DynamicImage,
    spec: &TransformSpec,
) -> Result<DynamicImage, AppError> {
    if let Some(c) = &spec.crop {
        if c.width == 0 || c.height == 0 {
            return Err(AppError::Other("crop width and height must be > 0".into()));
        }
        let (w, h) = (img.width(), img.height());
        if c.x >= w || c.y >= h {
            return Err(AppError::Other(format!(
                "crop origin ({}, {}) is outside the {w}x{h} image",
                c.x, c.y
            )));
        }
        // Clamp the rectangle to the image bounds instead of failing.
        let cw = c.width.min(w - c.x);
        let ch = c.height.min(h - c.y);
        img = img.crop_imm(c.x, c.y, cw, ch);
    }

    if spec.flip_horizontal.unwrap_or(false) {
        img = img.fliph();
    }
    if spec.flip_vertical.unwrap_or(false) {
        img = img.flipv();
    }

    match spec.rotate.unwrap_or(0) % 360 {
        0 => {}
        90 => img = img.rotate90(),
        180 => img = img.rotate180(),
        270 => img = img.rotate270(),
        other => {
            return Err(AppError::Other(format!(
                "rotate must be 0/90/180/270, got {other}"
            )))
        }
    }

    if let Some(r) = &spec.resize {
        img = resize_image(img, r)?;
    }

    Ok(img)
}

fn resize_image(img: DynamicImage, r: &ResizeSpec) -> Result<DynamicImage, AppError> {
    let filter = match r.filter.as_deref() {
        None | Some("lanczos3") => FilterType::Lanczos3,
        Some("nearest") => FilterType::Nearest,
        Some("triangle") => FilterType::Triangle,
        Some("catmullrom") => FilterType::CatmullRom,
        Some("gaussian") => FilterType::Gaussian,
        Some(other) => {
            return Err(AppError::Other(format!("unknown resize filter: {other}")))
        }
    };

    let exact = r.exact.unwrap_or(false);
    match (r.width, r.height) {
        (None, None) => Err(AppError::Other(
            "resize requires at least one of width / height".into(),
        )),
        // `resize` fits inside the box preserving aspect ratio; passing
        // u32::MAX on one axis pins the other axis exactly.
        (Some(w), None) => Ok(img.resize(w.max(1), u32::MAX, filter)),
        (None, Some(h)) => Ok(img.resize(u32::MAX, h.max(1), filter)),
        (Some(w), Some(h)) if exact => Ok(img.resize_exact(w.max(1), h.max(1), filter)),
        (Some(w), Some(h)) => Ok(img.resize(w.max(1), h.max(1), filter)),
    }
}

/// Decode `input`, apply `spec`, encode and write to `output`.
///
/// The EXIF orientation is normalised **first**, so the caller's `rotate` /
/// `flip` / `crop` operate on the same pixels the user sees in the main view
/// (which the engine auto-rotates). Without this, rotating a portrait phone
/// photo would appear to rotate the wrong axis. The output carries no EXIF, so
/// baking the orientation here cannot double-rotate.
pub fn transform_image(
    input: &Path,
    output: &Path,
    spec: &TransformSpec,
) -> Result<(), AppError> {
    let img = decode_oriented(input)?;
    let img = apply_transform(img, spec)?;
    let format = resolve_output_format(spec.format.as_deref(), output)?;
    let bytes = encode_image(&img, format, spec.quality)?;

    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    fs::write(output, bytes)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Multi-page / animation support
// ---------------------------------------------------------------------------

fn gif_decoder(
    path: &Path,
) -> Result<image::codecs::gif::GifDecoder<BufReader<fs::File>>, AppError> {
    let decoder =
        image::codecs::gif::GifDecoder::new(BufReader::new(fs::File::open(path)?))?;
    Ok(decoder)
}

fn webp_decoder(
    path: &Path,
) -> Result<image::codecs::webp::WebPDecoder<BufReader<fs::File>>, AppError> {
    let decoder =
        image::codecs::webp::WebPDecoder::new(BufReader::new(fs::File::open(path)?))?;
    Ok(decoder)
}

fn animation_frame_count(
    frames: impl Iterator<Item = image::ImageResult<image::Frame>>,
) -> Result<u32, AppError> {
    let mut count = 0u32;
    for frame in frames {
        frame?;
        count = count.saturating_add(1);
    }
    Ok(count)
}

const ANIMATION_CACHE_LIMIT: usize = 64 * 1024 * 1024;
const ANIMATION_CACHE_FRAMES: usize = 3;
const ANIMATION_COUNT_CACHE_ENTRIES: usize = 16;

struct AnimationSession {
    path: PathBuf,
    modified_ms: u64,
    file_size: u64,
    next_index: u32,
    frames: mpsc::Receiver<Result<image::Frame, String>>,
    cache: VecDeque<(u32, Arc<image::RgbaImage>)>,
    cache_bytes: usize,
}

static ANIMATION_SESSION: OnceLock<Mutex<Option<AnimationSession>>> = OnceLock::new();
static ANIMATION_COUNTS: OnceLock<Mutex<VecDeque<(PathBuf, u64, u64, u32)>>> = OnceLock::new();

fn cached_animation_frame_count(path: &Path, ext: &str) -> Result<u32, AppError> {
    let metadata = fs::metadata(path)?;
    let modified_ms = modified_ms_of(&metadata);
    let file_size = metadata.len();
    let cache = ANIMATION_COUNTS.get_or_init(|| Mutex::new(VecDeque::new()));
    {
        let mut cache = cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(position) = cache.iter().position(|(cached_path, modified, size, _)| {
            cached_path == path && *modified == modified_ms && *size == file_size
        }) {
            let entry = cache.remove(position).expect("cache position exists");
            let count = entry.3;
            cache.push_back(entry);
            return Ok(count);
        }
    }

    let count = match ext {
        "gif" => animation_frame_count(gif_decoder(path)?.into_frames())?,
        "webp" => animation_frame_count(webp_decoder(path)?.into_frames())?,
        _ => 1,
    };
    let mut cache = cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    cache.push_back((path.to_path_buf(), modified_ms, file_size, count));
    while cache.len() > ANIMATION_COUNT_CACHE_ENTRIES {
        cache.pop_front();
    }
    Ok(count)
}

fn stream_animation_frames(
    frames: impl Iterator<Item = image::ImageResult<image::Frame>>,
    sender: mpsc::SyncSender<Result<image::Frame, String>>,
) {
    for frame in frames {
        match frame {
            Ok(frame) => {
                if sender.send(Ok(frame)).is_err() {
                    return;
                }
            }
            Err(error) => {
                let _ = sender.send(Err(error.to_string()));
                return;
            }
        }
    }
}

fn start_animation_session(path: &Path, ext: &str) -> AnimationSession {
    let (sender, frames) = mpsc::sync_channel(1);
    let owned_path = path.to_path_buf();
    let owned_ext = ext.to_owned();
    std::thread::spawn(move || {
        let result: Result<(), AppError> = match owned_ext.as_str() {
            "gif" => match gif_decoder(&owned_path) {
                Ok(decoder) => {
                    stream_animation_frames(decoder.into_frames(), sender.clone());
                    return;
                }
                Err(error) => Err(error),
            },
            "webp" => match webp_decoder(&owned_path) {
                Ok(decoder) => {
                    stream_animation_frames(decoder.into_frames(), sender.clone());
                    return;
                }
                Err(error) => Err(error),
            },
            _ => Err(AppError::Unsupported(format!("animation format: {owned_ext}"))),
        };
        if let Err(error) = result {
            let _ = sender.send(Err(error.to_string()));
        }
    });
    let metadata = fs::metadata(path).ok();
    AnimationSession {
        path: path.to_path_buf(),
        modified_ms: metadata.as_ref().map(modified_ms_of).unwrap_or(0),
        file_size: metadata.as_ref().map(|m| m.len()).unwrap_or(0),
        next_index: 0,
        frames,
        cache: VecDeque::new(),
        cache_bytes: 0,
    }
}

fn cache_animation_frame(
    session: &mut AnimationSession,
    index: u32,
    frame: Arc<image::RgbaImage>,
) {
    let bytes = frame.as_raw().len();
    if bytes > ANIMATION_CACHE_LIMIT {
        return;
    }
    session.cache.push_back((index, frame));
    session.cache_bytes += bytes;
    while session.cache.len() > ANIMATION_CACHE_FRAMES
        || session.cache_bytes > ANIMATION_CACHE_LIMIT
    {
        if let Some((_, removed)) = session.cache.pop_front() {
            session.cache_bytes = session.cache_bytes.saturating_sub(removed.as_raw().len());
        }
    }
}

fn animation_frame_at(
    path: &Path,
    ext: &str,
    index: u32,
) -> Result<Arc<image::RgbaImage>, AppError> {
    let metadata = fs::metadata(path)?;
    let modified_ms = modified_ms_of(&metadata);
    let file_size = metadata.len();
    let slot = ANIMATION_SESSION.get_or_init(|| Mutex::new(None));
    let mut active = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let stale = active.as_ref().map_or(true, |session| {
        session.path != path || session.modified_ms != modified_ms || session.file_size != file_size
    });
    if stale {
        *active = Some(start_animation_session(path, ext));
    }
    let session = active.as_mut().expect("animation session initialized");

    if let Some(position) = session.cache.iter().position(|(cached, _)| *cached == index) {
        let item = session.cache.remove(position).expect("cache position exists");
        let frame = Arc::clone(&item.1);
        session.cache.push_back(item);
        return Ok(frame);
    }
    if index < session.next_index {
        *session = start_animation_session(path, ext);
    }
    let session = active.as_mut().expect("animation session initialized");
    while session.next_index <= index {
        let frame = match session.frames.recv() {
            Ok(Ok(frame)) => Arc::new(frame.into_buffer()),
            Ok(Err(error)) => return Err(AppError::Other(error)),
            Err(_) => return Err(AppError::Other(format!("frame index {index} out of range"))),
        };
        let current = session.next_index;
        session.next_index = session.next_index.saturating_add(1);
        cache_animation_frame(session, current, Arc::clone(&frame));
        if current == index {
            return Ok(frame);
        }
    }
    Err(AppError::Other(format!("frame index {index} out of range")))
}

fn tiff_error(e: tiff::TiffError) -> AppError {
    AppError::Other(format!("TIFF error: {e}"))
}

fn tiff_decoder(path: &Path) -> Result<tiff::decoder::Decoder<BufReader<fs::File>>, AppError> {
    tiff::decoder::Decoder::new(BufReader::new(fs::File::open(path)?)).map_err(tiff_error)
}

fn tiff_page_count(path: &Path) -> Result<u32, AppError> {
    let mut dec = tiff_decoder(path)?;
    let mut count = 1u32;
    while dec.more_images() {
        dec.next_image().map_err(tiff_error)?;
        count += 1;
    }
    Ok(count)
}

fn tiff_frame(path: &Path, index: u32) -> Result<DynamicImage, AppError> {
    let mut dec = tiff_decoder(path)?;
    for i in 0..index {
        if !dec.more_images() {
            return Err(AppError::Other(format!(
                "TIFF page index {index} out of range (stopped at page {i})"
            )));
        }
        dec.next_image().map_err(tiff_error)?;
    }
    // Orientation lives in the page's own IFD, so read it before consuming the
    // decoder. Output is a PNG, which the engine will not auto-rotate, so the
    // rotation has to be baked into the pixels here.
    let orientation = dec
        .get_tag_unsigned::<u16>(tiff::tags::Tag::Orientation)
        .ok()
        .and_then(|v| Orientation::from_exif(v as u8));

    let (w, h) = dec.dimensions().map_err(tiff_error)?;
    let color = dec.colortype().map_err(tiff_error)?;
    let data = dec.read_image().map_err(tiff_error)?;
    let mut img = tiff_to_dynamic(w, h, color, data)?;
    if let Some(orientation) = orientation {
        img.apply_orientation(orientation);
    }
    Ok(img)
}

fn tiff_to_dynamic(
    w: u32,
    h: u32,
    color: tiff::ColorType,
    data: tiff::decoder::DecodingResult,
) -> Result<DynamicImage, AppError> {
    use tiff::decoder::DecodingResult;

    // 16-bit samples are down-converted to 8 bit (Lumina displays 8-bit).
    fn narrow(samples: Vec<u16>) -> Vec<u8> {
        samples.into_iter().map(|s| (s >> 8) as u8).collect()
    }

    let buffer = match (color, data) {
        (tiff::ColorType::RGB(8), DecodingResult::U8(b)) => {
            image::RgbImage::from_raw(w, h, b).map(DynamicImage::ImageRgb8)
        }
        (tiff::ColorType::RGBA(8), DecodingResult::U8(b)) => {
            image::RgbaImage::from_raw(w, h, b).map(DynamicImage::ImageRgba8)
        }
        (tiff::ColorType::Gray(8), DecodingResult::U8(b)) => {
            image::GrayImage::from_raw(w, h, b).map(DynamicImage::ImageLuma8)
        }
        (tiff::ColorType::GrayA(8), DecodingResult::U8(b)) => {
            image::ImageBuffer::<image::LumaA<u8>, _>::from_raw(w, h, b)
                .map(DynamicImage::ImageLumaA8)
        }
        (tiff::ColorType::RGB(16), DecodingResult::U16(b)) => {
            image::RgbImage::from_raw(w, h, narrow(b)).map(DynamicImage::ImageRgb8)
        }
        (tiff::ColorType::RGBA(16), DecodingResult::U16(b)) => {
            image::RgbaImage::from_raw(w, h, narrow(b)).map(DynamicImage::ImageRgba8)
        }
        (tiff::ColorType::Gray(16), DecodingResult::U16(b)) => {
            image::GrayImage::from_raw(w, h, narrow(b)).map(DynamicImage::ImageLuma8)
        }
        (tiff::ColorType::GrayA(16), DecodingResult::U16(b)) => {
            image::ImageBuffer::<image::LumaA<u8>, _>::from_raw(w, h, narrow(b))
                .map(DynamicImage::ImageLumaA8)
        }
        (c, d) => {
            return Err(AppError::Unsupported(format!(
                "TIFF page color type not supported: {c:?} / {d:?}"
            )))
        }
    };
    buffer.ok_or_else(|| AppError::Other("TIFF page buffer size mismatch".into()))
}

/// Number of pages / frames in a file. Single-image formats return 1.
///
/// GIF and animated WebP are traversed one frame at a time without retaining
/// decoded buffers; multi-page TIFF reports its directory count.
pub fn page_count(path: &Path) -> Result<u32, AppError> {
    match extension_of(path).as_str() {
        "gif" => cached_animation_frame_count(path, "gif"),
        "webp" => cached_animation_frame_count(path, "webp"),
        "tif" | "tiff" => tiff_page_count(path),
        _ => Ok(1),
    }
}

/// Extract a single page / frame as PNG bytes.
///
/// The page's EXIF orientation is baked into the pixels: the response is a PNG
/// (no orientation tag), so the engine has nothing to auto-rotate.
pub fn read_frame_png(path: &Path, index: u32) -> Result<Vec<u8>, AppError> {
    let ext = extension_of(path);
    let mut img = match ext.as_str() {
        "gif" => {
            DynamicImage::ImageRgba8((*animation_frame_at(path, "gif", index)?).clone())
        }
        "webp" => {
            DynamicImage::ImageRgba8((*animation_frame_at(path, "webp", index)?).clone())
        }
        // `tiff_frame` already applies the per-page orientation tag.
        "tif" | "tiff" => tiff_frame(path, index)?,
        _ => {
            if index != 0 {
                return Err(AppError::Other(format!(
                    "file has a single page; index {index} is out of range"
                )));
            }
            decode(path)?
        }
    };
    if ext != "tif" && ext != "tiff" {
        apply_exif_orientation(&mut img, path);
    }
    encode_png(&img)
}

// ---------------------------------------------------------------------------
// File operations
// ---------------------------------------------------------------------------

/// Rename a file in place. `new_name` must be a plain file name (no path
/// separators) and must not collide with an existing entry.
pub fn rename_file(path: &Path, new_name: &str) -> Result<PathBuf, AppError> {
    if new_name.trim().is_empty() {
        return Err(AppError::Other("new name must not be empty".into()));
    }
    if new_name == "." || new_name == ".." || new_name.contains('/') || new_name.contains('\\') {
        return Err(AppError::Other(
            "new name must be a plain file name without path separators".into(),
        ));
    }
    if !path.is_file() {
        return Err(AppError::Other(format!("not a file: {}", path.display())));
    }
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Other("cannot rename a path without a parent directory".into()))?;
    let target = parent.join(new_name);
    if target.exists() {
        return Err(AppError::Other(format!(
            "target already exists: {}",
            target.display()
        )));
    }
    fs::rename(path, &target)?;
    Ok(target)
}

/// Delete a single file. Refuses to touch directories or symlinks so a stray
/// path can never wipe a whole folder.
pub fn delete_file(path: &Path) -> Result<(), AppError> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_file() {
        return Err(AppError::Other(format!(
            "refusing to delete a non-regular file: {}",
            path.display()
        )));
    }
    fs::remove_file(path)?;
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
        // `file_type()` comes from the directory entry itself (d_type on Unix),
        // avoiding the extra `stat` that `path.is_file()` would perform.
        if !entry.file_type()?.is_file() || !is_supported(&path) {
            continue;
        }
        let md = entry.metadata().ok();
        let file_size = md.as_ref().map(|m| m.len()).unwrap_or(0);
        let modified_ms = md.as_ref().map(modified_ms_of).unwrap_or(0);
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
            modified_ms,
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
        assert!(is_supported(Path::new("vector.svg")));
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
        assert_eq!(mime_for_ext("svg"), "image/svg+xml");
        assert_eq!(mime_for_ext("bin"), "application/octet-stream");
    }

    #[test]
    fn avif_and_heic_are_explicitly_unsupported() {
        assert!(matches!(parse_format("avif"), Err(AppError::Unsupported(_))));
        assert!(matches!(parse_format("heic"), Err(AppError::Unsupported(_))));
        assert!(parse_format("PNG").is_ok());
        assert!(parse_format(".jpg").is_ok());
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
        assert!(meta.modified_ms > 0, "mtime should be reported");

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
        assert!(listing.entries.iter().all(|e| e.modified_ms > 0));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn transform_order_crop_flip_rotate_resize() {
        let img = image::RgbImage::from_fn(40, 20, |x, y| {
            image::Rgb([(x * 6) as u8, (y * 12) as u8, 0])
        });
        let src = DynamicImage::ImageRgb8(img);

        // crop to 30x10, flip vertically, rotate 90, resize width to 15.
        let spec = TransformSpec {
            rotate: Some(90),
            flip_vertical: Some(true),
            crop: Some(CropRect { x: 10, y: 5, width: 30, height: 10 }),
            resize: Some(ResizeSpec { width: Some(15), ..Default::default() }),
            ..Default::default()
        };
        let out = apply_transform(src, &spec).unwrap();
        // 30x10 -> rotate90 -> 10x30 -> width 15 keeps aspect -> 15x45.
        assert_eq!((out.width(), out.height()), (15, 45));
    }

    #[test]
    fn transform_rejects_bad_inputs() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::new(10, 10));
        let bad_rotate = TransformSpec { rotate: Some(45), ..Default::default() };
        assert!(apply_transform(img.clone(), &bad_rotate).is_err());

        let no_size = TransformSpec {
            resize: Some(ResizeSpec::default()),
            ..Default::default()
        };
        assert!(apply_transform(img.clone(), &no_size).is_err());

        let zero_crop = TransformSpec {
            crop: Some(CropRect { x: 0, y: 0, width: 0, height: 5 }),
            ..Default::default()
        };
        assert!(apply_transform(img, &zero_crop).is_err());
    }

    #[test]
    fn jpeg_quality_changes_size() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_fn(64, 64, |x, y| {
            image::Rgb([(x * 3) as u8, (y * 3) as u8, ((x + y) * 2) as u8])
        }));
        let low = encode_image(&img, ImageFormat::Jpeg, Some(20)).unwrap();
        let high = encode_image(&img, ImageFormat::Jpeg, Some(95)).unwrap();
        assert!(high.len() > low.len());
    }

    #[test]
    fn rename_rejects_path_separators() {
        let dir = std::env::temp_dir().join(format!("lumina-rename-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.png");
        std::fs::write(&f, b"x").unwrap();

        assert!(rename_file(&f, "sub/b.png").is_err());
        assert!(rename_file(&f, "..").is_err());
        let renamed = rename_file(&f, "b.png").unwrap();
        assert!(renamed.is_file());
        assert!(!f.exists());

        std::fs::remove_dir_all(&dir).ok();
    }
}
