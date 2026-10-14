//! Format × result verification matrix.
//!
//! Runs the real pipeline against the committed fixtures in
//! `src-tauri/tests/fixtures/` and prints a table (`cargo test format_matrix --
//! --nocapture`). Every supported format is decoded, measured, thumbnailed and
//! frame-extracted; AVIF is asserted to fail with a clear error.

use std::path::{Path, PathBuf};

use image::DynamicImage;

use crate::imaging::{self, CropRect, ResizeSpec, TransformSpec};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures")
}

fn f(name: &str) -> PathBuf {
    fixtures().join(name)
}

/* -------------------------------------------------------------------------- */
/* Orientation helpers                                                         */
/* -------------------------------------------------------------------------- */

/// Which corner of a rendered image is red.
///
/// The `orient*.{jpg,tiff}` fixtures store a RED block in their *stored*
/// top-left corner over a dark blue field, so the applied orientation can be
/// read off directly: a 90-degree rotation moves the block to another corner.
/// Mirrors what `scripts/probe-webview2-orientation.cmd` measures in Blink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Corner {
    Tl,
    Tr,
    Bl,
    Br,
}

fn red_corner(img: &DynamicImage) -> Option<Corner> {
    let rgb = img.to_rgb8();
    let (w, h) = (rgb.width(), rgb.height());
    assert!(w >= 16 && h >= 16, "image too small for corner probing: {w}x{h}");

    // Average a 9x9 patch so JPEG ringing around the block edge cannot decide
    // the answer on its own.
    let patch = |cx: u32, cy: u32| -> (f64, f64, f64) {
        let (mut r, mut g, mut b, mut n) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
        for dy in -4i64..=4 {
            for dx in -4i64..=4 {
                let x = (cx as i64 + dx).clamp(0, w as i64 - 1) as u32;
                let y = (cy as i64 + dy).clamp(0, h as i64 - 1) as u32;
                let p = rgb.get_pixel(x, y);
                r += p[0] as f64;
                g += p[1] as f64;
                b += p[2] as f64;
                n += 1.0;
            }
        }
        (r / n, g / n, b / n)
    };
    let is_red = |(r, g, b): (f64, f64, f64)| r > 140.0 && g < 110.0 && b < 110.0;

    [
        (Corner::Tl, patch(5, 5)),
        (Corner::Tr, patch(w - 6, 5)),
        (Corner::Bl, patch(5, h - 6)),
        (Corner::Br, patch(w - 6, h - 6)),
    ]
    .into_iter()
    .find(|(_, c)| is_red(*c))
    .map(|(corner, _)| corner)
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

/// The frontend derives the *display* dimensions from `exif.orientation`
/// (the webview applies the rotation itself), so the orientation tag must be
/// readable for every container Lumina renders — not just JPEG.
#[test]
fn exif_orientation_is_reported_for_every_container() {
    println!("\n=== EXIF orientation is reported per container ===");
    for (name, expected) in [
        ("orient6.jpg", 6u16),
        ("orient8.jpg", 8),
        ("orient6.tiff", 6),
        ("orient8.tiff", 8),
    ] {
        let meta = imaging::read_metadata(&f(name)).unwrap();
        let orientation = meta.exif.as_ref().and_then(|e| e.orientation);
        println!(
            "{name:<14} orientation = {orientation:?}, stored = {}x{}",
            meta.width, meta.height
        );
        assert_eq!(orientation, Some(expected), "{name} orientation not reported");
    }
    println!("=== orientation matrix 通过 ===\n");
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

/* -------------------------------------------------------------------------- */
/* Phase 2C-B: EXIF orientation is actually applied                           */
/* -------------------------------------------------------------------------- */
//
// Reference: `scripts/probe-webview2-orientation.cmd` measures headless
// Edge/WebView2 (Blink 155.0.4283.45) against these very fixtures:
//
//     orient1.jpg natural=96x64 red=tl
//     orient6.jpg natural=64x96 red=tr   (engine rotated 90 CW)
//     orient8.jpg natural=64x96 red=bl   (engine rotated 270 CW)
//
// i.e. the WebView *does* honour the JPEG EXIF orientation for the main view.
// The Rust-side thumbnail therefore has to bake the same rotation in, or the
// thumbnail and the main view would disagree for portrait photos.

#[test]
fn exif_orientation_thumbnail_matches_webview() {
    // The `make_thumbnail` contract: downscale first, then rotate. A 90-degree
    // multiple is lossless so the order only matters for speed, not quality.
    println!("\n=== EXIF orientation: make_thumbnail ===");
    println!("{:<14} {:<12} {:<12} {:<10}", "fixture", "thumb size", "red corner", "expected");

    let cases: &[(&str, u32, u32, Corner)] = &[
        ("orient1.jpg", 48, 32, Corner::Tl),
        ("orient6.jpg", 32, 48, Corner::Tr),
        ("orient8.jpg", 32, 48, Corner::Bl),
    ];
    for (name, w, h, expect) in cases {
        let png = imaging::make_thumbnail(&f(name), 48)
            .unwrap_or_else(|e| panic!("thumbnail failed for {name}: {e}"));
        let thumb = image::load_from_memory(&png).unwrap();
        assert_eq!(
            (thumb.width(), thumb.height()),
            (*w, *h),
            "thumbnail dimensions for {name}"
        );
        let corner = red_corner(&thumb);
        assert_eq!(corner, Some(*expect), "red corner for {name}");
        println!(
            "{:<14} {:<12} {:<12} {:<10}",
            name,
            format!("{}x{}", thumb.width(), thumb.height()),
            format!("{:?}", corner.unwrap()),
            format!("{expect:?}"),
        );
    }
    println!("=== 缩略图方向与 WebView 一致 ===\n");
}

#[test]
fn exif_orientation_transcode_applies_before_encode() {
    // `read_for_display` for a *non*-webview-native format (TIFF) decodes,
    // bakes the orientation into the pixels, then encodes PNG — the PNG has no
    // EXIF block at all, so the engine cannot re-rotate it. Assert the pixels
    // are already oriented (red corner matches the WebView probe).
    println!("\n=== EXIF orientation: TIFF -> PNG transcode ===");
    let cases: &[(&str, Corner)] = &[("orient6.tiff", Corner::Tr), ("orient8.tiff", Corner::Bl)];
    for (name, expect) in cases {
        let (png, mime) = imaging::read_for_display(&f(name))
            .unwrap_or_else(|e| panic!("display failed for {name}: {e}"));
        assert_eq!(mime, "image/png", "TIFF must transcode to PNG");
        assert_eq!(&png[0..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        let img = image::load_from_memory(&png).unwrap();
        assert_eq!((img.width(), img.height()), (64, 96), "oriented size for {name}");
        let corner = red_corner(&img);
        assert_eq!(corner, Some(*expect), "red corner for {name}");
        println!(
            "{:<14} {:<12} {:<10}",
            name,
            format!("{}x{}", img.width(), img.height()),
            format!("{:?}", corner.unwrap()),
        );
    }
    println!("=== 转码路径已烘焙方向 ===\n");
}

#[test]
fn webview_native_passthrough_keeps_raw_bytes() {
    // For webview-native formats (JPEG here) `read_for_display` must stream the
    // *original* bytes: re-orienting in Rust would double-rotate (the engine
    // already applies the EXIF tag). Byte-for-byte equality proves no re-encode.
    println!("\n=== EXIF orientation: webview-native 直通不改字节 ===");
    for name in ["orient1.jpg", "orient6.jpg", "orient8.jpg"] {
        let path = f(name);
        let (bytes, mime) = imaging::read_for_display(&path).unwrap();
        let raw = std::fs::read(&path).unwrap();
        assert_eq!(mime, "image/jpeg", "{name} mime");
        assert_eq!(bytes, raw, "{name} bytes must be untouched");
        println!("{name:<14} passthrough {} bytes == raw file", bytes.len());
    }
    println!("=== JPEG 原样透传（引擎自行旋转）===\n");
}

#[test]
fn convert_and_transform_use_oriented_pixels() {
    // `convert_image` / `transform_image` decode through `decode_oriented`, so
    // the output carries the pixels already rotated and no stale orientation
    // tag. Converting orient6.jpg (rotate 90 CW) to PNG must yield a 64x96 PNG
    // with red at top-right — exactly what the `<img>` view shows.
    let out_dir = std::env::temp_dir().join("lumina-orient");
    std::fs::create_dir_all(&out_dir).unwrap();

    let out = out_dir.join("orient6.png");
    imaging::convert_image(&f("orient6.jpg"), &out, None, None).unwrap();
    let img = image::load_from_memory(&std::fs::read(&out).unwrap()).unwrap();
    assert_eq!((img.width(), img.height()), (64, 96), "convert normalises orientation");
    assert_eq!(red_corner(&img), Some(Corner::Tr));

    // transform_image on the same source sees an already-oriented 64x96 image,
    // so cropping the top-right (red) corner keeps the marker. After the 90 CW
    // rotation the 24x16 stored block sits at x in [48,64), y in [0,24).
    let crop = out_dir.join("crop.png");
    imaging::transform_image(
        &f("orient6.jpg"),
        &crop,
        &TransformSpec {
            crop: Some(CropRect { x: 44, y: 0, width: 20, height: 20 }),
            ..Default::default()
        },
    )
    .unwrap();
    let c = image::load_from_memory(&std::fs::read(&crop).unwrap()).unwrap();
    assert_eq!((c.width(), c.height()), (20, 20));
    assert!(red_corner(&c).is_some(), "top-right crop should contain the red marker");

    println!("\n=== convert/transform 统一按已定向像素处理 ===");
    println!("orient6.jpg -> PNG {}x{}, red=Tr", img.width(), img.height());
    println!("crop(44,0,20x20) 命中红色标记 = {}\n", red_corner(&c).is_some());

    std::fs::remove_dir_all(&out_dir).ok();
}
