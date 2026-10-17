//! Normalization of capture dates.
//!
//! Rules (TECHNICAL_SPEC §6):
//! - Keep the raw string, the local wall-clock value and the offset separately.
//! - A date without timezone is NOT treated as UTC automatically; year/month
//!   for folder naming come from the local wall-clock value.
//! - Filesystem mtime is only a fallback and must be explicitly marked.
//! - Never invent dates: unparsable/contradictory values become `NeedsReview`.

use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DateSource {
    /// EXIF DateTimeOriginal (+ OffsetTimeOriginal when present)
    Exif,
    /// Other explicit EXIF/XMP/IPTC date fields
    Xmp,
    Iptc,
    /// QuickTime / media container timestamps (usually UTC)
    MediaContainer,
    /// Other specialized dates returned by the metadata helper
    Specialized,
    /// Filesystem modification time — only when the fallback policy is on
    FileSystem,
    /// Manual override by the user
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DateStatus {
    /// A trustworthy capture time was found
    Confirmed,
    /// Found but ambiguous/unparsable/contradictory — needs user review
    NeedsReview,
    /// Only a filesystem-mtime fallback was used (marked in UI)
    Fallback,
    /// No trustworthy date at all
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapturedDate {
    /// Original string exactly as found in metadata
    pub captured_at_raw: Option<String>,
    /// Wall-clock value as recorded by the device (no timezone applied)
    pub captured_at_local: Option<NaiveDateTime>,
    /// UTC instant, only when the offset was known
    pub captured_at_utc: Option<DateTime<Utc>>,
    /// Offset in minutes east of UTC, when known
    pub tz_offset_min: Option<i16>,
    pub date_source: Option<DateSource>,
    pub date_status: DateStatus,
    /// Human explanation of how the value was normalized (stored in DB)
    pub normalization_note: Option<String>,
}

impl CapturedDate {
    pub fn unknown() -> Self {
        Self {
            captured_at_raw: None,
            captured_at_local: None,
            captured_at_utc: None,
            tz_offset_min: None,
            date_source: None,
            date_status: DateStatus::Unknown,
            normalization_note: None,
        }
    }

    /// Year/month used for folder naming — from the local wall-clock value.
    pub fn year_month(&self) -> Option<(i32, u32)> {
        self.captured_at_local.map(|dt| (dt.year(), dt.month()))
    }
}

use chrono::{Datelike, Timelike};

/// Parse an EXIF-style date "YYYY:MM:DD HH:MM:SS" (also tolerates ISO dashes/slashes).
pub fn parse_exif_datetime(raw: &str, offset_raw: Option<&str>) -> Option<NaiveDateTime> {
    let cleaned = raw.trim().replace('/', "-");
    // EXIF uses ':' as date separator: 2025:08:12 14:03:22
    let normalized = if cleaned.len() >= 10 && cleaned.as_bytes()[4] == b':' {
        format!("{}-{}-{}", &cleaned[0..4], &cleaned[5..7], &cleaned[8..10]) + &cleaned[10..]
    } else {
        cleaned
    };
    let naives = [
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d",
    ];
    for fmt in naives {
        if let Ok(dt) = NaiveDateTime::parse_from_str(&normalized, fmt) {
            return Some(dt);
        }
    }
    // date-only parsed above by NaiveDate path
    if let Ok(d) = chrono::NaiveDate::parse_from_str(&normalized, "%Y-%m-%d") {
        return d.and_hms_opt(0, 0, 0);
    }
    let _ = offset_raw; // offset handled by the caller to keep local value pure
    None
}

/// Parse "+03:00" / "-0500" style offsets used by EXIF OffsetTimeOriginal.
pub fn parse_offset(offset_raw: &str) -> Option<i16> {
    let s = offset_raw.trim();
    let (sign, rest) = match s.as_bytes().first() {
        Some(b'+') => (1i16, &s[1..]),
        Some(b'-') => (-1i16, &s[1..]),
        _ => return None,
    };
    let (hh, mm) = match rest.find(':') {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None if rest.len() == 4 => (&rest[..2], &rest[2..]),
        None if rest.len() == 2 => (rest, "00"),
        _ => return None,
    };
    let hh: i16 = hh.parse().ok()?;
    let mm: i16 = mm.parse().ok()?;
    if hh > 23 || mm > 59 {
        return None;
    }
    Some(sign * (hh * 60 + mm))
}

/// Parse an ISO-8601 timestamp used by media containers (ffprobe `creation_time`,
/// usually UTC with trailing Z).
pub fn parse_container_datetime(raw: &str) -> Option<(NaiveDateTime, Option<i16>)> {
    let s = raw.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        let local = dt.naive_local();
        return Some((local, Some(dt.offset().local_minus_utc() as i16)));
    }
    // Some tags are bare "YYYY-MM-DDTHH:MM:SS" documented as UTC.
    if let Ok(naive) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S"))
    {
        return Some((naive, None));
    }
    // QuickTime tags often use the EXIF layout "2024:02:29 21:10:05"
    parse_exif_datetime(s, None).map(|n| (n, None))
}

pub fn utc_from_local_offset(local: NaiveDateTime, offset_min: i16) -> Option<DateTime<Utc>> {
    Utc.timestamp_opt(
        local.and_utc().timestamp() - offset_min as i64 * 60,
        local.nanosecond(),
    )
    .single()
}

/// Build a CapturedDate out of EXIF DateTimeOriginal + OffsetTimeOriginal.
pub fn from_exif(raw: &str, offset_raw: Option<&str>) -> CapturedDate {
    let trimmed = raw.trim();
    if trimmed.starts_with("0000") || trimmed.is_empty() {
        return CapturedDate {
            captured_at_raw: Some(raw.to_string()),
            date_source: Some(DateSource::Exif),
            date_status: DateStatus::NeedsReview,
            normalization_note: Some("EXIF-дата порожня або заповнена нулями".into()),
            ..CapturedDate::unknown()
        };
    }
    match parse_exif_datetime(trimmed, offset_raw) {
        Some(local) => {
            let offset = offset_raw.and_then(parse_offset);
            CapturedDate {
                captured_at_raw: Some(raw.to_string()),
                tz_offset_min: offset,
                captured_at_local: Some(local),
                captured_at_utc: offset.and_then(|o| utc_from_local_offset(local, o)),
                date_source: Some(DateSource::Exif),
                date_status: DateStatus::Confirmed,
                normalization_note: Some(match offset {
                    Some(_) => "Дата з EXIF DateTimeOriginal разом з OffsetTimeOriginal".into(),
                    None => {
                        "Дата з EXIF DateTimeOriginal без часового поясу; рік/місяць взято з локального часу зйомки".into()
                    }
                }),
            }
        }
        None => CapturedDate {
            captured_at_raw: Some(raw.to_string()),
            date_source: Some(DateSource::Exif),
            date_status: DateStatus::NeedsReview,
            normalization_note: Some("EXIF-date не розпізнано; потрібна перевірка".into()),
            ..CapturedDate::unknown()
        },
    }
}

/// Build a CapturedDate from a media container tag (ffprobe/QuickTime creation_time).
pub fn from_container(raw: &str) -> CapturedDate {
    match parse_container_datetime(raw) {
        Some((local, offset)) => CapturedDate {
            captured_at_raw: Some(raw.to_string()),
            captured_at_local: Some(local),
            tz_offset_min: offset,
            captured_at_utc: offset.and_then(|o| utc_from_local_offset(local, o)),
            date_source: Some(DateSource::MediaContainer),
            date_status: DateStatus::Confirmed,
            normalization_note: Some(
                "Дата з медіаконтейнера; теги можуть бути в UTC або локальному часі".into(),
            ),
        },
        None => CapturedDate {
            captured_at_raw: Some(raw.to_string()),
            date_source: Some(DateSource::MediaContainer),
            date_status: DateStatus::NeedsReview,
            normalization_note: Some("Дата контейнера не розпізнано; потрібна перевірка".into()),
            ..CapturedDate::unknown()
        },
    }
}

/// Filesystem mtime fallback — only when the user enabled the policy.
pub fn from_filesystem(mtime_epoch: i64) -> CapturedDate {
    let dt = Utc.timestamp_opt(mtime_epoch, 0).single();
    match dt {
        Some(utc) => CapturedDate {
            captured_at_raw: Some(utc.to_rfc3339()),
            captured_at_utc: Some(utc),
            captured_at_local: Some(utc.naive_utc()),
            tz_offset_min: Some(0),
            date_source: Some(DateSource::FileSystem),
            date_status: DateStatus::Fallback,
            normalization_note: Some(
                "Використано дату модифікації файлової системи, а не підтверджений час зйомки"
                    .into(),
            ),
        },
        None => CapturedDate {
            date_source: Some(DateSource::FileSystem),
            date_status: DateStatus::NeedsReview,
            normalization_note: Some("Некоректна дата модифікації файлу".into()),
            ..CapturedDate::unknown()
        },
    }
}

/// Manual user override.
pub fn from_manual(raw: &str) -> CapturedDate {
    match parse_exif_datetime(raw, None) {
        Some(local) => CapturedDate {
            captured_at_raw: Some(raw.to_string()),
            captured_at_local: Some(local),
            captured_at_utc: None,
            tz_offset_min: None,
            date_source: Some(DateSource::Manual),
            date_status: DateStatus::Confirmed,
            normalization_note: Some("Дата призначена вручну".into()),
        },
        None => CapturedDate {
            captured_at_raw: Some(raw.to_string()),
            date_source: Some(DateSource::Manual),
            date_status: DateStatus::NeedsReview,
            normalization_note: Some("Введену дату не вдалося розпізнати".into()),
            ..CapturedDate::unknown()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exif_with_offset() {
        let d = from_exif("2025:08:12 14:03:22", Some("+03:00"));
        assert_eq!(d.date_status, DateStatus::Confirmed);
        let local = d.captured_at_local.unwrap();
        assert_eq!((local.year(), local.month()), (2025, 8));
        let utc = d.captured_at_utc.unwrap();
        assert_eq!((utc.hour(), utc.day()), (11, 12));
        assert_eq!(d.tz_offset_min, Some(180));
    }

    #[test]
    fn exif_without_offset_is_not_utc() {
        let d = from_exif("2025:12:31 23:30:00", None);
        assert_eq!(d.date_status, DateStatus::Confirmed);
        assert!(d.captured_at_utc.is_none());
        assert_eq!(d.captured_at_local.unwrap().year(), 2025);
        assert_eq!(d.captured_at_local.unwrap().month(), 12);
    }

    #[test]
    fn exif_zero_is_needs_review() {
        let d = from_exif("0000:00:00 00:00:00", None);
        assert_eq!(d.date_status, DateStatus::NeedsReview);
        assert!(d.captured_at_local.is_none());
    }

    #[test]
    fn exif_garbage_is_needs_review() {
        let d = from_exif("not a date", None);
        assert_eq!(d.date_status, DateStatus::NeedsReview);
    }

    #[test]
    fn container_utc() {
        let d = from_container("2024-02-29T21:10:05Z");
        assert_eq!(d.date_status, DateStatus::Confirmed);
        assert_eq!(d.tz_offset_min, Some(0));
        let l = d.captured_at_local.unwrap();
        assert_eq!((l.year(), l.month(), l.day()), (2024, 2, 29));
    }

    #[test]
    fn fs_fallback_marked() {
        let d = from_filesystem(1754995402);
        assert_eq!(d.date_status, DateStatus::Fallback);
        assert_eq!(d.date_source, Some(DateSource::FileSystem));
    }

    #[test]
    fn offsets_parse() {
        assert_eq!(parse_offset("+03:00"), Some(180));
        assert_eq!(parse_offset("-0500"), Some(-300));
        assert_eq!(parse_offset("+00:00"), Some(0));
        assert_eq!(parse_offset("bogus"), None);
    }

    #[test]
    fn year_month_uses_local() {
        // 23:30 local +03:00 => UTC is still 20:30 same day; folder must use local
        let d = from_exif("2025:12:31 23:30:00", Some("+03:00"));
        assert_eq!(d.year_month(), Some((2025, 12)));
    }
}
