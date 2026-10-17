//! End-to-end acceptance tests against the real ExifTool binary (§18.3, §18.4).
//! Skipped gracefully when exiftool is not installed on the test machine.

use kadr_core::domain::captured_date::{DateSource, DateStatus};
use kadr_core::persistence::db::{self, MediaFilter};
use kadr_core::services::metadata::ExifTool;
use kadr_core::services::scanner::{scan_root, ScanOptions};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;

#[test]
fn real_exif_date_and_gps() {
    let exif = ExifTool::new();
    if exif.bin.is_none() {
        eprintln!("exiftool not installed — skipping");
        return;
    }
    let dir = std::env::temp_dir().join(format!("kadr-e2e-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let jpg = dir.join("IMG_2048.jpg");
    let img = image::DynamicImage::new_rgb8(16, 12);
    img.save(&jpg).unwrap();
    // stamp EXIF date with offset and GPS coordinates
    let status = Command::new(exif.bin.as_ref().unwrap())
        .args([
            "-overwrite_original",
            "-EXIF:DateTimeOriginal=2025:08:12 14:03:22",
            "-EXIF:OffsetTimeOriginal=+03:00",
            "-EXIF:GPSLatitude=48.537878",
            "-EXIF:GPSLongitude=37.92",
        ])
        .arg(&jpg)
        .status()
        .unwrap();
    assert!(status.success());

    let conn = db::open_db(":memory:").unwrap();
    let root = db::add_source_root(&conn, dir.to_str().unwrap()).unwrap();
    let cancel = AtomicBool::new(false);
    scan_root(&conn, &root, &ScanOptions::default(), "j", &cancel, &|_| {}).unwrap();

    let (items, total) = db::query_media(&conn, &MediaFilter::default()).unwrap();
    assert_eq!(total, 1, "{items:?}");
    let it = &items[0];
    // §18.3: EXIF date read and year/month correct
    assert_eq!(it.date_status, DateStatus::Confirmed);
    assert_eq!(it.date_source, Some(DateSource::Exif));
    assert_eq!(it.captured_at_local.as_deref(), Some("2025-08-12T14:03:22"));
    assert_eq!(it.timezone_offset, Some(180));
    // §18.4: GPS stored with provenance
    assert!(it.latitude.unwrap() - 48.537878 < 1e-6);
    assert_eq!(it.place_status, kadr_core::domain::location::PlaceStatus::CoordsOnly);
    // §18.5: no invented place name
    assert!(it.place_label.is_none());

    std::fs::remove_dir_all(&dir).ok();
    let _ = Path::new("");
}
