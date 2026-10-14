//! Rasterise `assets/file-icons/*.svg` into multi-resolution Windows `.ico`
//! files under `src-tauri/icons/file-icons/`.
//!
//!     cd src-tauri && cargo run --bin gen-file-icons
//!
//! This is a **development tool**, not part of the app: the generated `.ico`
//! files are committed and shipped as bundle resources, exactly like the app
//! icons produced by `pnpm tauri icon`. Keeping the rasterisation out of the
//! runtime means the registry never points at a file the app has to synthesise
//! on first run.
//!
//! The SVG sources are font-free (glyphs are stroke paths), so
//! `usvg::Options::default()` is enough and the output is byte-identical on
//! every machine -- no fontdb, no system font lookup.

use std::path::Path;

use image::codecs::ico::{IcoEncoder, IcoFrame};
use image::ExtendedColorType;
use resvg::{tiny_skia, usvg};

/// Sizes Explorer actually asks for. 16/24/32 cover the small views, 48/64 the
/// medium ones, 256 the extra-large tiles. A missing size makes Explorer
/// upscale a neighbour, which is where blurry file icons come from.
const SIZES: [u32; 6] = [16, 24, 32, 48, 64, 256];

/// Same order as `file_assoc::FORMATS`; one icon per wordmark.
const KEYS: [&str; 8] = ["png", "jpg", "gif", "webp", "bmp", "tiff", "ico", "svg"];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("no workspace root")?;
    let src_dir = root.join("assets/file-icons");
    let out_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("icons/file-icons");
    std::fs::create_dir_all(&out_dir)?;

    for key in KEYS {
        let svg_path = src_dir.join(format!("{key}.svg"));
        let ico = build_ico(&svg_path)?;
        let out = out_dir.join(format!("{key}.ico"));
        std::fs::write(&out, &ico)?;
        println!("{key:>5}.ico  {:>6} bytes  ({} sizes)", ico.len(), SIZES.len());
    }
    Ok(())
}

/// Render one SVG at every size in [`SIZES`] and pack the frames into an ICO.
fn build_ico(svg_path: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let data = std::fs::read(svg_path)?;
    let tree = usvg::Tree::from_data(&data, &usvg::Options::default())?;
    let intrinsic = tree.size().width();
    if intrinsic <= 0.0 {
        return Err(format!("{}: zero intrinsic width", svg_path.display()).into());
    }

    // `pixmap.encode_png()` writes a correctly (un)premultiplied RGBA PNG, so
    // the frames can go straight into the ICO without touching pixel data --
    // `IcoFrame::with_encoded` takes the pre-encoded bytes as-is.
    let mut frames = Vec::with_capacity(SIZES.len());
    for size in SIZES {
        let scale = size as f32 / intrinsic;
        let mut pixmap = tiny_skia::Pixmap::new(size, size)
            .ok_or_else(|| format!("{size}x{size} pixmap allocation failed"))?;
        resvg::render(
            &tree,
            tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        let png = pixmap.encode_png()?;
        frames.push(IcoFrame::with_encoded(
            png,
            size,
            size,
            ExtendedColorType::Rgba8,
        )?);
    }

    let mut out = Vec::new();
    IcoEncoder::new(&mut out).encode_images(&frames)?;
    Ok(out)
}
