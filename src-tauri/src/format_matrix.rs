//! Format × result verification matrix.
//!
//! Runs the real pipeline against the committed fixtures in
//! `src-tauri/tests/fixtures/` and prints a table (`cargo test format_matrix --
//! --nocapture`). Every supported format is decoded, measured, thumbnailed and
//! frame-extracted; AVIF is asserted to fail with a clear error.

use std::path::{Path, PathBuf};

use crate::imaging::{self, CropRect, ResizeSpec, TransformSpec};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures")
}

fn f(name: &str) -> PathBuf {
    fixtures().join(name)
}

/// (fixture, expected normalised format, expected dimensions)
///
/// `grad.ico` holds several resolutions (96/48/32/16); the decoder reports the
/// largest, hence 96x96.
const RASTER: &[(&str, &str, u32, u32)] = &[
    ("grad.png", "png", 96, 64),
    ("grad.jpg", "jpg", 96, 64),
    ("grad.bmp", "bmp", 96, 64),
    ("grad.tiff", "tiff", 96, 64),
    ("grad.webp", "webp", 96, 64),
    ("grad.ico", "ico", 96, 96),
    ("shape.svg", "svg", 120, 80),
];

#[test]
fn format_matrix() {
    println!("\n=== Lumina 格式支持矩阵 (真实固件解码) ===");
    println!("{:<16} {:<8} {:<12} {:<10} {:<9} {:<9}", "file", "format", "dimensions", "metadata", "display", "thumbnail");

    for (name, expect_fmt, w, h) in RASTER {
        let path = f(name);
        let meta = imaging::read_metadata(&path)
            .unwrap_or_else(|e| panic!("metadata failed for {name}: {e}"));
        assert_eq!(meta.format, *expect_fmt, "format mismatch for {name}");
        assert_eq!((meta.width, meta.height), (*w, *h), "dimensions for {name}");

        let img = imaging::decode(&path).unwrap_or_else(|e| panic!("decode failed for {name}: {e}"));
        assert!(img.width() > 0 && img.height() > 0);

        let (_, mime) = imaging::read_for_display(&path)
            .unwrap_or_else(|e| panic!("display failed for {name}: {e}"));

        let thumb = imaging::make_thumbnail(&path, 32)
            .unwrap_or_else(|e| panic!("thumbnail failed for {name}: {e}"));
        let t = image::load_from_memory(&thumb)
            .unwrap_or_else(|e| panic!("thumbnail not decodable for {name}: {e}"));
        assert!(t.width() <= 32 && t.height() <= 32);

        println!(
            "{:<16} {:<8} {:<12} {:<10} {:<9} {:<9}",
            name,
            meta.format,
            format!("{}x{}", meta.width, meta.height),
            "OK",
            mime,
            format!("{}x{}", t.width(), t.height()),
        );
    }
    println!("=== 全部 {} 种基础格式解码通过 ===\n", RASTER.len());
}

#[test]
fn animation_and_multipage_matrix() {
    println!("\n=== 多页 / 动画矩阵 ===");
    println!("{:<18} {:<7} {:<8} {:<12}", "file", "pages", "frames?", "frame0 size");

    let cases: &[(&str, u32)] = &[
        ("anim.gif", 3),
        ("anim.webp", 3),
        ("multipage.tiff", 3),
        ("grad.png", 1),
    ];
    for (name, expect_pages) in cases {
        let path = f(name);
        let pages = imaging::page_count(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(pages, *expect_pages, "page count for {name}");

        let mut sizes = Vec::new();
        for i in 0..pages {
            let png = imaging::read_frame_png(&path, i)
                .unwrap_or_else(|e| panic!("{name} frame {i}: {e}"));
            let frame = image::load_from_memory(&png)
                .unwrap_or_else(|e| panic!("{name} frame {i} not a PNG: {e}"));
            assert_eq!(&png[0..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
            sizes.push(format!("{}x{}", frame.width(), frame.height()));
        }
        // Out-of-range frame must be a clean error.
        assert!(imaging::read_frame_png(&path, pages).is_err());

        println!("{:<18} {:<7} {:<8} {:<12}", name, pages, if pages > 1 { "yes" } else { "-" }, sizes.join(","));
    }
    println!("=== 多页导航通过 ===\n");
}

#[test]
fn palette_tiff_navigation_but_no_pixel_decode() {
    // Documented limitation: the `tiff` crate rejects indexed (palette) and
    // bilevel pages at the *pixel* level, yet page counting / navigation still
    // works because it only walks directories.
    let path = f("palette.tiff");
    let pages = imaging::page_count(&path).expect("page count should work for palette TIFF");
    assert_eq!(pages, 3);
    let err = imaging::read_frame_png(&path, 0)
        .expect_err("palette TIFF pixel decode is expected to fail");
    println!("\n=== palette.tiff 限制 ===");
    println!("pages = {pages} (导航可用), frame 解码 -> Err: {err}");
    println!("=== 记录为已知限制（见 docs/INTERFACES.md 8.）===\n");
}

#[test]
fn exif_matrix() {
    println!("\n=== EXIF 读取矩阵 (photo.jpg) ===");
    let meta = imaging::read_metadata(&f("photo.jpg")).unwrap();
    let exif = meta.exif.expect("photo.jpg should carry EXIF");
    assert_eq!(exif.make.as_deref(), Some("LuminaCam"));
    assert_eq!(exif.model.as_deref(), Some("LC-1000"));
    assert_eq!(exif.datetime.as_deref(), Some("2024:05:06 07:08:09"));
    assert_eq!(exif.orientation, Some(6));
    assert!(exif.lens_model.is_some(), "lens model missing");

    let lat = exif.gps_latitude.expect("gps_latitude missing");
    let lon = exif.gps_longitude.expect("gps_longitude missing");
    // 40°26'46" N
    assert!((lat - (40.0 + 26.0 / 60.0 + 46.0 / 3600.0)).abs() < 1e-6, "lat={lat}");
    // 116°23'29" E
    assert!((lon - (116.0 + 23.0 / 60.0 + 29.0 / 3600.0)).abs() < 1e-6, "lon={lon}");

    println!("file            = photo.jpg");
    println!("make/model      = {:?} / {:?}", exif.make, exif.model);
    println!("datetime        = {:?}", exif.datetime);
    println!("orientation     = {:?}", exif.orientation);
    println!("lens            = {:?}", exif.lens_model);
    println!("software        = {:?}", exif.software);
    println!("artist/copyright= {:?} / {:?}", exif.artist, exif.copyright);
    println!("exposure/f/iso  = {:?} / {:?} / {:?}", exif.exposure_time, exif.f_number, exif.iso);
    println!("focal_length    = {:?}", exif.focal_length);
    println!("gps             = {lat:.6}, {lon:.6} alt={:?}", exif.gps_altitude);
    println!("=== EXIF 通过 ===\n");
}

#[test]
fn avif_is_reported_unsupported() {
    // Fixture exists (ImageMagick wrote it) but Lumina has no pure-Rust AVIF
    // decoder, so the failure must be a clean error rather than a panic.
    let path = f("grad.avif");
    assert!(path.is_file(), "grad.avif fixture missing");
    let err = imaging::decode(&path);
    assert!(err.is_err(), "AVIF unexpectedly decoded");
    println!("\n=== AVIF 评估 ===");
    println!("grad.avif decode -> Err (expected): {}", err.unwrap_err());
    println!("=== AVIF 推迟，原因见 docs/INTERFACES.md ===\n");
}

#[test]
fn operations_matrix() {
    println!("\n=== 图像操作矩阵 (grad.png 96x64) ===");
    let src = f("grad.png");
    let out_dir = std::env::temp_dir().join("lumina-ops");
    std::fs::create_dir_all(&out_dir).unwrap();

    struct Case {
        label: &'static str,
        spec: TransformSpec,
        expect: (u32, u32),
        ext: &'static str,
    }
    let cases = vec![
        Case { label: "rotate90", spec: TransformSpec { rotate: Some(90), ..Default::default() }, expect: (64, 96), ext: "png" },
        Case { label: "rotate180", spec: TransformSpec { rotate: Some(180), ..Default::default() }, expect: (96, 64), ext: "png" },
        Case { label: "rotate270", spec: TransformSpec { rotate: Some(270), ..Default::default() }, expect: (64, 96), ext: "png" },
        Case { label: "flip_h", spec: TransformSpec { flip_horizontal: Some(true), ..Default::default() }, expect: (96, 64), ext: "png" },
        Case { label: "flip_v", spec: TransformSpec { flip_vertical: Some(true), ..Default::default() }, expect: (96, 64), ext: "png" },
        Case { label: "crop", spec: TransformSpec { crop: Some(CropRect { x: 10, y: 10, width: 30, height: 20 }), ..Default::default() }, expect: (30, 20), ext: "png" },
        Case { label: "resize_w", spec: TransformSpec { resize: Some(ResizeSpec { width: Some(48), ..Default::default() }), ..Default::default() }, expect: (48, 32), ext: "png" },
        Case { label: "resize_fit", spec: TransformSpec { resize: Some(ResizeSpec { width: Some(48), height: Some(48), ..Default::default() }), ..Default::default() }, expect: (48, 32), ext: "png" },
        Case { label: "resize_exact", spec: TransformSpec { resize: Some(ResizeSpec { width: Some(40), height: Some(40), exact: Some(true), ..Default::default() }), ..Default::default() }, expect: (40, 40), ext: "png" },
        Case { label: "to_jpeg_q40", spec: TransformSpec { format: Some("jpeg".into()), quality: Some(40), ..Default::default() }, expect: (96, 64), ext: "jpg" },
        Case { label: "to_webp", spec: TransformSpec { format: Some("webp".into()), ..Default::default() }, expect: (96, 64), ext: "webp" },
        Case { label: "to_bmp", spec: TransformSpec { format: Some("bmp".into()), ..Default::default() }, expect: (96, 64), ext: "bmp" },
    ];

    println!("{:<14} {:<12} {:<10} {:<8}", "op", "result", "expected", "file");
    for (i, c) in cases.iter().enumerate() {
        let out = out_dir.join(format!("op{i}.{}", c.ext));
        imaging::transform_image(&src, &out, &c.spec)
            .unwrap_or_else(|e| panic!("{} failed: {e}", c.label));
        let meta = imaging::read_metadata(&out).unwrap();
        assert_eq!((meta.width, meta.height), c.expect, "{} dims", c.label);
        assert!(out.is_file() && meta.file_size > 0);
        println!(
            "{:<14} {:<12} {:<10} {:<8}",
            c.label,
            format!("{}x{}", meta.width, meta.height),
            format!("{}x{}", c.expect.0, c.expect.1),
            out.file_name().unwrap().to_string_lossy()
        );
    }

    // JPEG quality actually affects the encoded size.
    let q20 = imaging::transform_image(&src, &out_dir.join("q20.jpg"), &TransformSpec { format: Some("jpeg".into()), quality: Some(20), ..Default::default() });
    let q95 = imaging::transform_image(&src, &out_dir.join("q95.jpg"), &TransformSpec { format: Some("jpeg".into()), quality: Some(95), ..Default::default() });
    assert!(q20.is_ok() && q95.is_ok());
    let s20 = std::fs::metadata(out_dir.join("q20.jpg")).unwrap().len();
    let s95 = std::fs::metadata(out_dir.join("q95.jpg")).unwrap().len();
    assert!(s95 > s20, "quality 95 ({s95}) should be larger than 20 ({s20})");
    println!("jpeg quality 20 -> {s20} bytes, quality 95 -> {s95} bytes");
    println!("=== 图像操作通过 ===\n");

    std::fs::remove_dir_all(&out_dir).ok();
}
