//! The point of the built-in native EXIF reader: JPEG/TIFF/HEIF files still
//! get DATE + GPS when ExifTool is unavailable (degraded install, unsupported
//! file, etc.). Fixture is stamped with real ExifTool when present; the READ
//! path uses the native reader only.

use kadr_core::services::metadata::{ExifTool, MetadataReader, NativeImage};
use std::process::Command;

#[test]
fn native_reader_extracts_date_and_gps_without_exiftool() {
    let exif = ExifTool::new();
    let Some(bin) = exif.bin else {
        eprintln!("exiftool not installed — skipping fixture stamping");
        return;
    };
    let dir = std::env::temp_dir().join(format!("kadr-native-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let jpg = dir.join("IMG_1000.jpg");
    image::DynamicImage::new_rgb8(16, 12).save(&jpg).unwrap();
    let ok = Command::new(&bin)
        .args([
            "-overwrite_original",
            "-EXIF:DateTimeOriginal=2025:08:12 14:03:22",
            "-EXIF:OffsetTimeOriginal=+03:00",
            "-EXIF:GPSLatitude=48.858",
            "-EXIF:GPSLatitudeRef=N",
            "-EXIF:GPSLongitude=2.294",
            "-EXIF:GPSLongitudeRef=E",
        ])
        .arg(&jpg)
        .status()
        .unwrap()
        .success();
    assert!(ok, "exiftool stamping failed");

    let m = NativeImage
        .read(&jpg)
        .unwrap()
        .expect("native reader should return metadata");

    let c = m.captured.expect("date must be read natively");
    let local = c.captured_at_local.expect("local time");
    assert_eq!(
        local.format("%Y-%m-%d %H:%M:%S").to_string(),
        "2025-08-12 14:03:22"
    );
    assert_eq!(c.tz_offset_min, Some(180), "UTC offset parsed from EXIF");

    let g = m.geo.expect("gps must be read natively");
    assert!((g.latitude - 48.858).abs() < 1e-3, "lat {}", g.latitude);
    assert!((g.longitude - 2.294).abs() < 1e-3, "lon {}", g.longitude);

    assert_eq!(m.width, Some(16));
    assert_eq!(m.height, Some(12));
    let _ = std::fs::remove_dir_all(dir);
}
