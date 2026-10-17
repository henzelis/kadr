//! Folder template parsing, path building and sanitization (TECHNICAL_SPEC §8).

use serde::{Deserialize, Serialize};

pub const DEFAULT_TEMPLATE: &str = "{year}/{month_number} - {month_name}/{place}/{filename}";
pub const DEFAULT_UNKNOWN_DATE: &str = "Невідома дата";
pub const DEFAULT_NO_GEO: &str = "Без геолокації";

pub const UA_MONTHS: [&str; 12] = [
    "Січень",
    "Лютий",
    "Березень",
    "Квітень",
    "Травень",
    "Червень",
    "Липень",
    "Серпень",
    "Вересень",
    "Жовтень",
    "Листопад",
    "Грудень",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MonthFormat {
    /// "08 - Серпень"
    #[default]
    NumberName,
    /// "08"
    Number,
    /// "Серпень"
    Name,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetOs {
    Windows,
    Unix,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateConfig {
    pub template: String,
    pub unknown_date_label: String,
    pub no_geo_label: String,
    pub month_format: MonthFormat,
    pub max_segment_len: usize,
    pub target_os: TargetOs,
    /// When GPS exists but no label, use "lat, lon" as the place label.
    pub coords_as_label: bool,
}

impl Default for TemplateConfig {
    fn default() -> Self {
        Self {
            template: DEFAULT_TEMPLATE.to_string(),
            unknown_date_label: DEFAULT_UNKNOWN_DATE.to_string(),
            no_geo_label: DEFAULT_NO_GEO.to_string(),
            month_format: MonthFormat::NumberName,
            max_segment_len: 80,
            target_os: current_os(),
            coords_as_label: false,
        }
    }
}

pub fn current_os() -> TargetOs {
    if cfg!(windows) {
        TargetOs::Windows
    } else {
        TargetOs::Unix
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    Literal(String),
    Year,
    MonthNumber,
    MonthName,
    Place,
    Filename,
}

/// Parse a template string into tokens. '/' stays inside Literal tokens.
pub fn parse_template(t: &str) -> Result<Vec<Token>, String> {
    let mut tokens: Vec<Token> = Vec::new();
    let mut cur = String::new();
    let mut chars = t.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            if !cur.is_empty() {
                tokens.push(Token::Literal(std::mem::take(&mut cur)));
            }
            let mut name = String::new();
            let mut closed = false;
            while let Some(&c) = chars.peek() {
                chars.next();
                if c == '}' {
                    closed = true;
                    break;
                }
                if c == '{' {
                    return Err("вкладені дужки в шаблоні".into());
                }
                name.push(c);
            }
            if !closed {
                return Err("незакрита дужка в шаблоні".into());
            }
            tokens.push(match name.as_str() {
                "year" => Token::Year,
                "month_number" => Token::MonthNumber,
                "month_name" => Token::MonthName,
                "place" => Token::Place,
                "filename" => Token::Filename,
                other => return Err(format!("невідома змінна «{other}» у шаблоні")),
            });
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        tokens.push(Token::Literal(cur));
    }
    let last_meaningful = tokens
        .iter()
        .rev()
        .find(|t| !matches!(t, Token::Literal(s) if s.trim_matches('/') == ""));
    match last_meaningful {
        Some(Token::Filename) => Ok(tokens),
        _ => Err("шаблон повинен завершуватися змінною {filename}".into()),
    }
}

fn month_name(m: u32) -> &'static str {
    UA_MONTHS
        .get(m.saturating_sub(1) as usize)
        .copied()
        .unwrap_or("?")
}

/// Characters forbidden in file/folder names (both are replaced with space).
fn is_forbidden(c: char, os: TargetOs) -> bool {
    match os {
        TargetOs::Windows => matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'),
        TargetOs::Unix => matches!(c, '/' | '\\'),
    }
}

const WIN_RESERVED: [&str; 24] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9", "COM0", "LPT0",
];

/// Sanitize one path segment. Never returns "..", ".", empty or anything
/// containing separators; blocks Windows reserved names and trailing dots.
pub fn sanitize_segment(seg: &str, os: TargetOs, max_len: usize) -> String {
    let replaced: String = seg
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| if is_forbidden(c, os) { ' ' } else { c })
        .collect();
    // any remaining dot-dot component becomes literal dots-with-space
    let parts: Vec<String> = replaced
        .split_whitespace()
        .map(|w| if w == ".." || w == "." { format!("_{w}_") } else { w.to_string() })
        .collect();
    let mut s = parts.join(" ");
    s = s.trim().trim_matches('.').trim().to_string();
    if matches!(os, TargetOs::Windows) {
        let stem_upper = s.split('.').next().unwrap_or("").to_ascii_uppercase();
        if WIN_RESERVED.contains(&stem_upper.as_str()) {
            s = format!("_{s}");
        }
    }
    if s.chars().count() > max_len {
        s = s.chars().take(max_len).collect::<String>();
        s = s.trim().trim_matches('.').trim().to_string();
    }
    s
}

/// Trim dangling separators left when an optional token produced nothing.
fn clean_separators(s: &str) -> String {
    let seps = [' ', '-', '–', '_'];
    let mut s = s.trim().to_string();
    s = s.trim_matches(seps).trim().to_string();
    while s.contains("  ") {
        s = s.replace("  ", " ");
    }
    while s.contains(" - - ") || s.starts_with("- ") || s.ends_with(" -") {
        s = s.replace(" - - ", " - ").trim_matches(seps).trim().to_string();
    }
    s
}

/// Input for building one destination path.
#[derive(Debug, Clone)]
pub struct PathInput<'a> {
    pub year_month: Option<(i32, u32)>,
    pub place: Option<&'a str>,
    pub coords_label: Option<&'a str>,
    pub filename: &'a str,
}

/// Build sanitized destination segments (relative to the archive root).
/// Guarantees: non-empty result, last segment = sanitized filename,
/// no segment is ".."/"."/absolute, no separator inside any segment.
pub fn build_dest_segments(cfg: &TemplateConfig, input: &PathInput) -> Vec<String> {
    let tokens = match parse_template(&cfg.template) {
        Ok(t) => t,
        Err(_) => parse_template(DEFAULT_TEMPLATE).expect("default template is valid"),
    };
    let (year, month) = match input.year_month {
        Some(ym) => (Some(ym.0), Some(ym.1)),
        None => (None, None),
    };
    let place_raw: String = match (input.place, input.coords_label) {
        (Some(p), _) if !p.trim().is_empty() => p.trim().to_string(),
        (None, Some(c)) if cfg.coords_as_label => c.to_string(),
        _ => cfg.no_geo_label.clone(),
    };

    // Assemble one string per template segment (split literals on '/').
    let mut raw_segments: Vec<String> = Vec::new();
    let mut cur = String::new();
    {
        let mut push_split = |buf: &mut String| {
            for piece in buf.split('/') {
                raw_segments.push(piece.to_string());
            }
            buf.clear();
        };
        for tok in &tokens {
            match tok {
                Token::Literal(s) => {
                    if s.contains('/') {
                        let mut it = s.split('/');
                        cur.push_str(it.next().unwrap_or(""));
                        push_split(&mut cur);
                        for mid in it {
                            cur.push_str(mid);
                        }
                        // trailing part stays in cur; if s ended with '/', cur ends empty
                        if s.ends_with('/') {
                            push_split(&mut cur);
                        }
                    } else {
                        cur.push_str(s);
                    }
                }
                Token::Year => match year {
                    Some(y) => cur.push_str(&y.to_string()),
                    None => cur.push_str(&cfg.unknown_date_label),
                },
                Token::MonthNumber => {
                    if let Some(m) = month {
                        if cfg.month_format != MonthFormat::Name {
                            cur.push_str(&format!("{m:02}"));
                        }
                    }
                }
                Token::MonthName => {
                    if let Some(m) = month {
                        match cfg.month_format {
                            MonthFormat::Number => {}
                            MonthFormat::Name | MonthFormat::NumberName => {
                                cur.push_str(month_name(m))
                            }
                        }
                    }
                }
                Token::Place => cur.push_str(&place_raw),
                Token::Filename => cur.push_str(input.filename),
            }
        }
        push_split(&mut cur);
    }

    let mut out: Vec<String> = raw_segments
        .iter()
        .map(|s| sanitize_segment(&clean_separators(s), cfg.target_os, cfg.max_segment_len))
        .filter(|s| !s.is_empty())
        .collect();
    if out.last().map(|s| s.as_str()) != Some(sanitize_segment(input.filename, cfg.target_os, cfg.max_segment_len).as_str()) {
        let fname = sanitize_segment(input.filename, cfg.target_os, cfg.max_segment_len);
        let fname = if fname.is_empty() { "file".to_string() } else { fname };
        out.push(fname);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> TemplateConfig {
        TemplateConfig {
            target_os: TargetOs::Windows,
            ..Default::default()
        }
    }

    fn segs(input: PathInput) -> Vec<String> {
        build_dest_segments(&cfg(), &input)
    }

    #[test]
    fn default_layout() {
        let s = segs(PathInput {
            year_month: Some((2025, 8)),
            place: Some("Карпати"),
            coords_label: None,
            filename: "IMG_2048.HEIC",
        });
        assert_eq!(s, vec!["2025", "08 - Серпень", "Карпати", "IMG_2048.HEIC"]);
    }

    #[test]
    fn unknown_date_and_place() {
        let s = segs(PathInput {
            year_month: None,
            place: None,
            coords_label: None,
            filename: "x.jpg",
        });
        assert_eq!(s, vec!["Невідома дата", "Без геолокації", "x.jpg"]);
    }

    #[test]
    fn no_date_with_place() {
        let s = segs(PathInput {
            year_month: None,
            place: Some("Париж"),
            coords_label: None,
            filename: "p.jpg",
        });
        assert_eq!(s, vec!["Невідома дата", "Париж", "p.jpg"]);
    }

    #[test]
    fn month_format_number() {
        let mut c = cfg();
        c.month_format = MonthFormat::Number;
        let s = build_dest_segments(
            &c,
            &PathInput { year_month: Some((2025, 8)), place: Some("K"), coords_label: None, filename: "f" },
        );
        assert_eq!(s, vec!["2025", "08", "K", "f"]);
    }

    #[test]
    fn month_format_name() {
        let mut c = cfg();
        c.month_format = MonthFormat::Name;
        let s = build_dest_segments(
            &c,
            &PathInput { year_month: Some((2025, 12)), place: Some("K"), coords_label: None, filename: "f" },
        );
        assert_eq!(s, vec!["2025", "Грудень", "K", "f"]);
    }

    #[test]
    fn sanitize_windows_rules() {
        assert_eq!(sanitize_segment("funny: name?<>|*", TargetOs::Windows, 80), "funny name");
        assert_eq!(sanitize_segment("CON.txt", TargetOs::Windows, 80), "_CON.txt");
        assert_eq!(sanitize_segment("ends with dots...", TargetOs::Windows, 80), "ends with dots");
        assert_eq!(sanitize_segment("..", TargetOs::Windows, 80), "_.._");
        assert_eq!(sanitize_segment("a/b\\c", TargetOs::Windows, 80), "a b c");
    }

    #[test]
    fn sanitize_keeps_unicode() {
        assert_eq!(
            sanitize_segment("Київ '24 — поїздка", TargetOs::Windows, 80),
            "Київ '24 — поїздка"
        );
    }

    #[test]
    fn traversal_blocked() {
        let s = segs(PathInput {
            year_month: Some((2025, 1)),
            place: Some("../../etc"),
            coords_label: None,
            filename: "evil.jpg",
        });
        assert!(!s.iter().any(|seg| seg == ".." || seg == "."));
        assert!(s.iter().all(|seg| !seg.contains('/') && !seg.contains('\\')));
        assert_eq!(*s.last().unwrap(), "evil.jpg");
    }

    #[test]
    fn long_segment_truncated() {
        let long_place = "д".repeat(120);
        let s = segs(PathInput {
            year_month: Some((2025, 1)),
            place: Some(&long_place),
            coords_label: None,
            filename: "f.jpg",
        });
        assert_eq!(s[2].chars().count(), 80);
    }

    #[test]
    fn template_rejects_missing_filename() {
        assert!(parse_template("{year}/{place}").is_err());
        assert!(parse_template("{year}/{bogus}/{filename}").is_err());
        assert!(parse_template("{year}/{place}/{filename}").is_ok());
    }

    #[test]
    fn custom_template() {
        let mut c = cfg();
        c.template = "{place}/{year}/{filename}".into();
        let s = build_dest_segments(
            &c,
            &PathInput { year_month: Some((2024, 3)), place: Some("Львів"), coords_label: None, filename: "a.jpg" },
        );
        assert_eq!(s, vec!["Львів", "2024", "a.jpg"]);
    }

    #[test]
    fn coords_label_used() {
        let mut c = cfg();
        c.coords_as_label = true;
        let s = build_dest_segments(
            &c,
            &PathInput { year_month: Some((2025, 1)), place: None, coords_label: Some("48.53800, 37.92000"), filename: "a.jpg" },
        );
        assert_eq!(s[2], "48.53800, 37.92000");
    }
}
