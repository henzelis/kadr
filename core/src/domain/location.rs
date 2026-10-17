//! Location / geotag model (TECHNICAL_SPEC §7).
//! GPS coordinates are NOT a place name. Without a local geo base or manual
//! input the status stays "coords only".

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocationSource {
    /// Manual override by the user — never overwritten by rescans
    Manual,
    /// Explicit textual location fields in metadata (XMP City, IPTC, ...)
    Metadata,
    /// Reverse geocoding from an offline local database
    Geocoder,
    /// Raw coordinates used as label (can be disabled)
    Coords,
    /// Nothing available
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaceStatus {
    /// A human readable place label is known
    Resolved,
    /// GPS present but the place could not be resolved
    CoordsOnly,
    /// No GPS and no label at all
    NoGeo,
    /// Metadata present but ambiguous — needs review
    NeedsReview,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeoPoint {
    pub latitude: f64,
    pub longitude: f64,
    pub altitude: Option<f64>,
}

impl GeoPoint {
    pub fn valid(&self) -> bool {
        self.latitude.is_finite()
            && self.longitude.is_finite()
            && self.latitude >= -90.0
            && self.latitude <= 90.0
            && self.longitude >= -180.0
            && self.longitude <= 180.0
            // (0,0) in the Gulf of Guinea is almost always junk data
            && !(self.latitude == 0.0 && self.longitude == 0.0)
    }
    /// Coordinate label like "48.538, 37.920" used as fallback/inspector text.
    pub fn label(&self) -> String {
        format!("{:.5}, {:.5}", self.latitude, self.longitude)
    }
}

/// Parse EXIF GPS degrees/minutes/seconds + refs into a point.
/// exiftool -n already yields signed decimals, this helper covers DMS strings.
pub fn parse_dms(deg: f64, minutes: f64, seconds: f64, ref_letter: &str) -> Option<f64> {
    if !deg.is_finite() || !minutes.is_finite() || !seconds.is_finite() {
        return None;
    }
    let v = deg + minutes / 60.0 + seconds / 3600.0;
    match ref_letter.trim().to_ascii_uppercase().as_str() {
        "S" | "W" => Some(-v),
        "N" | "E" => Some(v),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_range() {
        assert!(GeoPoint {
            latitude: 48.5,
            longitude: 37.9,
            altitude: None
        }
        .valid());
        assert!(!GeoPoint {
            latitude: 91.0,
            longitude: 0.0,
            altitude: None
        }
        .valid());
        assert!(!GeoPoint {
            latitude: 0.0,
            longitude: 0.0,
            altitude: None
        }
        .valid());
        assert!(!GeoPoint {
            latitude: f64::NAN,
            longitude: 1.0,
            altitude: None
        }
        .valid());
    }

    #[test]
    fn dms_south_west() {
        assert_eq!(parse_dms(48.0, 0.0, 0.0, "N"), Some(48.0));
        let s = parse_dms(33.0, 51.0, 0.0, "S").unwrap();
        assert!((s - -33.85).abs() < 1e-9);
        assert_eq!(parse_dms(1.0, 0.0, 0.0, "X"), None);
    }
}
