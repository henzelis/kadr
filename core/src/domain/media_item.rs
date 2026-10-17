use crate::domain::captured_date::{DateSource, DateStatus};
use crate::domain::location::{LocationSource, PlaceStatus};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Photo,
    Video,
    Sidecar,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataStatus {
    /// Metadata read successfully
    Ok,
    /// Metadata helper unavailable / read failed
    Unavailable,
    /// Some fields are ambiguous or need review
    NeedsReview,
    /// Unsupported format for this build
    Unsupported,
}

/// One indexed media file (mirrors `media_item` table).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaItem {
    pub id: String,
    pub source_root_id: String,
    pub path: String,
    pub normalized_path: String,
    pub file_name: String,
    pub extension: String,
    pub media_kind: MediaKind,

    pub size_bytes: i64,
    pub mtime: i64,
    pub file_identity: Option<String>,
    pub sha256: Option<String>,
    pub fingerprint_updated_at: Option<i64>,

    pub mime_or_detected_type: Option<String>,
    pub metadata_status: MetadataStatus,
    pub thumbnail_path: Option<String>,

    pub captured_at_raw: Option<String>,
    pub captured_at_local: Option<String>, // RFC3339-ish naive
    pub captured_at_utc: Option<String>,
    pub timezone_offset: Option<i16>,
    pub date_source: Option<DateSource>,
    pub date_status: DateStatus,
    pub normalization_note: Option<String>,

    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub altitude: Option<f64>,
    pub place_label: Option<String>,
    pub location_source: Option<LocationSource>,
    pub place_status: PlaceStatus,
    pub manual_place_override: bool,

    pub duration_ms: Option<i64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,

    pub scan_timestamp: i64,
    pub last_error: Option<String>,
    pub is_sidecar: bool,
    pub sidecar_parent_id: Option<String>,
}

impl MediaItem {
    pub fn has_reliable_date(&self) -> bool {
        matches!(self.date_status, DateStatus::Confirmed) && self.captured_at_local.is_some()
    }
    pub fn is_needs_review(&self) -> bool {
        matches!(self.date_status, DateStatus::NeedsReview)
            || matches!(self.place_status, PlaceStatus::NeedsReview)
            || matches!(self.metadata_status, MetadataStatus::NeedsReview)
    }
}

/// Recognized extensions → kind. The extension is only a hint; detection is
/// confirmed by metadata/signature when available (TECHNICAL_SPEC §5).
pub fn kind_from_extension(ext: &str) -> MediaKind {
    let e = ext.to_ascii_lowercase();
    match e.as_str() {
        "jpg" | "jpeg" | "png" | "webp" | "gif" | "bmp" | "tif" | "tiff" | "heic" | "heif"
        | "avif" | "jxl" | "ico" => MediaKind::Photo,
        "dng" | "cr2" | "cr3" | "nef" | "nrw" | "arw" | "raf" | "orf" | "rw2" | "pef" | "srw" => {
            MediaKind::Photo
        }
        "mp4" | "mov" | "m4v" | "mkv" | "avi" | "webm" | "3gp" | "3g2" | "mpeg" | "mpg" | "mts"
        | "m2ts" | "ts" | "wmv" | "flv" | "ogv" => MediaKind::Video,
        "aae" | "aa" | "xmp" | "mp" | "jpg.xml" => MediaKind::Sidecar,
        _ => MediaKind::Other,
    }
}

pub fn is_supported_media(ext: &str) -> bool {
    !matches!(kind_from_extension(ext), MediaKind::Other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_mapping() {
        assert_eq!(kind_from_extension("JPG"), MediaKind::Photo);
        assert_eq!(kind_from_extension("heic"), MediaKind::Photo);
        assert_eq!(kind_from_extension("cr3"), MediaKind::Photo);
        assert_eq!(kind_from_extension("MOV"), MediaKind::Video);
        assert_eq!(kind_from_extension("aae"), MediaKind::Sidecar);
        assert_eq!(kind_from_extension("zip"), MediaKind::Other);
        assert!(!is_supported_media("exe"));
        assert!(is_supported_media("pdf") == false);
    }
}
