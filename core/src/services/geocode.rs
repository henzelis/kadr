// Offline reverse geocoding at city scale (GeoNames cities500, CC-BY 4.0).
//
// The gazetteer ships as a TSV resource inside the app bundle (~11 MB, 236k
// places). Resolution is a plain nearest-neighbour scan: microseconds per
// unique coordinate, no network, no database dependency. A manual place
// assignment always wins over the automatic label.

use std::path::PathBuf;
use std::sync::OnceLock;

pub struct Place {
    pub lat: f64,
    pub lon: f64,
    pub population: i64,
    pub label: String,
}

pub struct GeoBase {
    places: Vec<Place>,
}

impl GeoBase {
    pub fn parse(tsv: &str) -> GeoBase {
        let mut places = Vec::new();
        for line in tsv.lines() {
            if line.starts_with("geonameid\t") || line.trim().is_empty() {
                continue;
            }
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < 6 {
                continue;
            }
            let (Ok(lat), Ok(lon), Ok(pop)) = (
                f[1].parse::<f64>(),
                f[2].parse::<f64>(),
                f[3].parse::<i64>(),
            ) else {
                continue;
            };
            // Ukrainian name when known, ASCII fallback otherwise
            let label = if !f[5].is_empty() {
                f[5].to_string()
            } else {
                f[4].to_string()
            };
            if !label.is_empty() {
                places.push(Place {
                    lat,
                    lon,
                    population: pop,
                    label,
                });
            }
        }
        GeoBase { places }
    }

    /// Nearest place by equirectangular distance, with a ~150 km cutoff so
    /// remote/ocean coordinates honestly resolve to nothing.
    pub fn nearest(&self, lat: f64, lon: f64) -> Option<&Place> {
        let cos = lat.to_radians().cos();
        let mut best: Option<(f64, &Place)> = None;
        for p in &self.places {
            let dy = p.lat - lat;
            let dx = (p.lon - lon) * cos;
            let d2 = dy * dy + dx * dx;
            match best {
                Some((bd, _)) if d2 >= bd => {}
                _ => best = Some((d2, p)),
            }
        }
        // 150 km ≈ 1.35 degrees of latitude
        best.filter(|(d2, _)| *d2 < 1.35 * 1.35).map(|(_, p)| p)
    }
}

fn geobase_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(p) = std::env::var("KADR_GEOBASE") {
        paths.push(PathBuf::from(p));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            paths.push(dir.join("resources").join("geobase.tsv"));
            paths.push(dir.join("geobase.tsv"));
        }
    }
    paths.push(PathBuf::from("resources/geobase.tsv"));
    paths.push(PathBuf::from("src-tauri/resources/geobase.tsv"));
    paths
}

static GEOBASE: OnceLock<Option<GeoBase>> = OnceLock::new();

/// Resolve coordinates to a human-readable city label, offline.
pub fn resolve(lat: f64, lon: f64) -> Option<String> {
    GEOBASE
        .get_or_init(|| {
            geobase_candidates()
                .iter()
                .find(|p| p.is_file())
                .and_then(|p| std::fs::read_to_string(p).ok())
                .map(|s| GeoBase::parse(&s))
        })
        .as_ref()
        .and_then(|b| b.nearest(lat, lon))
        .map(|p| p.label.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = "geonameid\tlat\tlon\tpopulation\tname_ascii\tname_uk\n\
703448\t50.45466\t30.5238\t2952301\tKyiv\tКиїв\n\
695859\t48.05472\t24.20314\t15621\tRakhiv\tРахів\n\
702007\t49.83968\t24.02972\t727665\tLviv\tЛьвів\n\
3189368\t42.26637\t18.87287\t11077\tBudva\t\t\n";

    #[test]
    fn nearest_uses_ukrainian_labels() {
        let b = GeoBase::parse(FIXTURE);
        // demo photo coordinates: 48.9, 24.7 → Rakhiv region
        let p = b.nearest(48.9, 24.7).expect("carpathian coords resolve");
        assert_eq!(p.label, "Рахів");
        let k = b.nearest(50.45, 30.52).expect("kyiv coords resolve");
        assert_eq!(k.label, "Київ");
    }

    #[test]
    fn ascii_fallback_when_no_ukrainian_name() {
        let b = GeoBase::parse(FIXTURE);
        let p = b.nearest(42.27, 18.87).expect("budva resolves");
        assert_eq!(p.label, "Budva");
    }

    #[test]
    fn ocean_resolves_to_nothing() {
        let b = GeoBase::parse(FIXTURE);
        assert!(b.nearest(30.0, -50.0).is_none());
    }
}
