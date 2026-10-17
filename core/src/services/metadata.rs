//! Metadata providers behind a common trait (TECHNICAL_SPEC §15).
//! ExifTool is the primary helper; FFprobe covers video containers; a native
//! image reader is the last-resort fallback. All process calls use explicit
//! argument lists — never shell interpolation.

use crate::domain::captured_date::{self, CapturedDate};
use crate::domain::location::GeoPoint;
use crate::errors::{KResult, KadrError};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use wait_timeout::ChildExt;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MediaMetadata {
    pub captured: Option<CapturedDate>,
    pub geo: Option<GeoPoint>,
    pub place_text: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_ms: Option<i64>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub mime: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolStatus {
    pub exiftool_path: Option<String>,
    pub exiftool_version: Option<String>,
    pub ffprobe_path: Option<String>,
    pub ffprobe_version: Option<String>,
}

/// Directories searched for bundled helpers, in order:
/// KADR_HELPER_DIR (set by the shell to its resource dir), then dirs next to
/// the executable (portable/AppImage layouts).
fn helper_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(d) = std::env::var("KADR_HELPER_DIR") {
        dirs.push(std::path::PathBuf::from(d));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.join("helpers"));
            dirs.push(dir.join("resources").join("helpers"));
        }
    }
    dirs
}

/// Resolution order: explicit KADR_<BIN> override -> bundled helper shipped
/// with the app -> system PATH. A bundled helper always wins over whatever
/// happens to be on the user's PATH, so behavior is identical on every machine.
fn locate(bin: &str) -> Option<String> {
    if let Ok(p) = std::env::var(format!("KADR_{}", bin.to_uppercase())) {
        if Path::new(&p).exists() {
            return Some(p);
        }
    }
    for dir in helper_dirs() {
        for base in [dir.clone(), dir.join(bin)] {
            for cand in [base.join(bin), base.join(format!("{bin}.exe"))] {
                if cand.is_file() {
                    return Some(cand.to_string_lossy().into_owned());
                }
            }
        }
    }
    which(bin)
}

/// GUI apps on Windows flash a console window for every spawned child —
/// CREATE_NO_WINDOW keeps ExifTool/FFprobe silent even across thousands of calls.
pub(crate) fn no_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

fn which(bin: &str) -> Option<String> {
    let mut probe = Command::new(if cfg!(windows) { "where" } else { "which" });
    probe.arg(bin).stdout(Stdio::piped()).stderr(Stdio::null());
    no_window(&mut probe);
    let out = probe.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let first = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()?
        .trim()
        .to_string();
    if first.is_empty() {
        None
    } else {
        Some(first)
    }
}

pub fn detect_tools() -> ToolStatus {
    let mut st = ToolStatus {
        exiftool_path: locate("exiftool"),
        exiftool_version: None,
        ffprobe_path: locate("ffprobe"),
        ffprobe_version: None,
    };
    if let Some(p) = &st.exiftool_path {
        st.exiftool_version = run_capture(p, &["-ver"], 10).map(|v| v.trim().to_string());
    }
    if let Some(p) = &st.ffprobe_path {
        st.ffprobe_version = run_capture(p, &["-version"], 10)
            .and_then(|v| v.lines().next().map(|l| l.trim().to_string()));
    }
    st
}

/// Spawn a helper process with piped stdout/stderr read on background
/// threads (avoids pipe-buffer deadlock), killed on timeout.
pub(crate) fn run_captured(
    cmd: &mut Command,
    timeout: Duration,
) -> std::io::Result<(Option<std::process::ExitStatus>, Vec<u8>, Vec<u8>)> {
    use std::io::Read;
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let mut so = child.stdout.take().unwrap();
    let mut se = child.stderr.take().unwrap();
    let out_thread = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = so.read_to_end(&mut v);
        v
    });
    let err_thread = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = se.read_to_end(&mut v);
        v
    });
    let status = child.wait_timeout(timeout)?;
    if status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let stdout = out_thread.join().unwrap_or_default();
    let stderr = err_thread.join().unwrap_or_default();
    Ok((status, stdout, stderr))
}

fn run_capture(bin: &str, args: &[&str], timeout_s: u64) -> Option<String> {
    let mut cmd = Command::new(bin);
    no_window(&mut cmd);
    cmd.args(args);
    let (status, stdout, _) = run_captured(&mut cmd, Duration::from_secs(timeout_s)).ok()?;
    if !status.map(|s| s.success()).unwrap_or(false) {
        return None;
    }
    String::from_utf8(stdout).ok()
}

pub trait MetadataReader: Send + Sync {
    fn name(&self) -> &'static str;
    fn is_available(&self) -> bool;
    /// Read metadata for one file; Ok(None) means "no metadata found".
    fn read(&self, path: &Path) -> KResult<Option<MediaMetadata>>;
}

// ---------------------------------------------------------------------------
// ExifTool adapter
// ---------------------------------------------------------------------------

pub struct ExifTool {
    pub bin: Option<String>,
}

impl ExifTool {
    pub fn new() -> Self {
        Self {
            bin: locate("exiftool"),
        }
    }

    /// Batch-read several files in one process call (spec §9: no per-file subprocess storm).
    pub fn read_batch(&self, paths: &[PathBuf]) -> Vec<(PathBuf, KResult<MediaMetadata>)> {
        let bin = match &self.bin {
            Some(b) => b.clone(),
            None => {
                return paths
                    .iter()
                    .map(|p| {
                        (
                            p.clone(),
                            Err(KadrError::Helper {
                                tool: "ExifTool".into(),
                                reason: "не знайдено у PATH".into(),
                            }),
                        )
                    })
                    .collect()
            }
        };
        let mut cmd = Command::new(&bin);
        no_window(&mut cmd);
        cmd.arg("-json").arg("-n").arg("-G1").arg("-a");
        for p in paths {
            cmd.arg(p);
        }
        let wait = run_captured(&mut cmd, Duration::from_secs(120));
        let (status_opt, stdout, _stderr) = match wait {
            Ok(t) => t,
            Err(e) => {
                return paths
                    .iter()
                    .map(|p| {
                        (
                            p.clone(),
                            Err(KadrError::Helper {
                                tool: "ExifTool".into(),
                                reason: format!("не вдалося запустити: {e}"),
                            }),
                        )
                    })
                    .collect()
            }
        };
        if status_opt.is_none() {
            // The batch hit its wall-clock limit: report it, never retry-storm.
            return paths
                .iter()
                .map(|p| {
                    (
                        p.clone(),
                        Err(KadrError::user_detail(
                            "ExifTool не встиг відповісти (таймаут)",
                            p.display().to_string(),
                        )),
                    )
                })
                .collect();
        }
        // ExifTool exits non-zero on plain warnings — the JSON output decides
        // success. Only files the batch skipped get one individual retry,
        // otherwise a folder full of warnings turns into thousands of spawns.
        let objects = parse_exiftool_json(&String::from_utf8_lossy(&stdout)).unwrap_or_default();
        let mut by_file: std::collections::HashMap<
            String,
            serde_json::Map<String, serde_json::Value>,
        > = objects.into_iter().map(|o| (source_file(&o), o)).collect();
        let mut out = Vec::with_capacity(paths.len());
        for p in paths {
            let key = p.to_string_lossy().to_string();
            if let Some(obj) = by_file.remove(&key) {
                out.push((p.clone(), Ok(exiftool_to_metadata(&obj))));
                continue;
            }
            let one = self.bin.as_ref().unwrap();
            let mut sc = Command::new(one);
            no_window(&mut sc);
            sc.arg("-json")
                .arg("-n")
                .arg("-G1")
                .arg("-a")
                .arg(p.as_path());
            let objs = match run_captured(&mut sc, Duration::from_secs(60)) {
                Ok((_, stdout1, _)) => {
                    parse_exiftool_json(&String::from_utf8_lossy(&stdout1)).unwrap_or_default()
                }
                _ => Vec::new(),
            };
            if let Some(obj) = objs.into_iter().next() {
                out.push((p.clone(), Ok(exiftool_to_metadata(&obj))));
            } else {
                out.push((
                    p.clone(),
                    Err(KadrError::user_detail(
                        "Формат не підтримується або файл пошкоджений",
                        p.display().to_string(),
                    )),
                ));
            }
        }
        out
    }
}

impl Default for ExifTool {
    fn default() -> Self {
        Self::new()
    }
}

impl MetadataReader for ExifTool {
    fn name(&self) -> &'static str {
        "exiftool"
    }
    fn is_available(&self) -> bool {
        self.bin.is_some()
    }
    fn read(&self, path: &Path) -> KResult<Option<MediaMetadata>> {
        self.read_batch(&[path.to_path_buf()])
            .into_iter()
            .next()
            .map(|(_, r)| r.map(Some))
            .unwrap_or(Ok(None))
    }
}

fn source_file(obj: &serde_json::Map<String, serde_json::Value>) -> String {
    obj.get("SourceFile")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

pub fn parse_exiftool_json(
    text: &str,
) -> Result<Vec<serde_json::Map<String, serde_json::Value>>, serde_json::Error> {
    let v: serde_json::Value = serde_json::from_str(text)?;
    match v {
        serde_json::Value::Array(arr) => Ok(arr
            .into_iter()
            .filter_map(|x| x.as_object().cloned())
            .collect()),
        serde_json::Value::Object(o) => Ok(vec![o]),
        _ => Ok(Vec::new()),
    }
}

fn get_str<'a>(o: &'a serde_json::Map<String, serde_json::Value>, key: &str) -> Option<&'a str> {
    o.get(key).and_then(|v| v.as_str())
}
fn get_any_str<'a>(
    o: &'a serde_json::Map<String, serde_json::Value>,
    keys: &[&'a str],
) -> Option<&'a str> {
    keys.iter().find_map(|k| get_str(o, k))
}
fn get_f64(o: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<f64> {
    o.get(key).and_then(|v| v.as_f64())
}
fn get_any_f64(o: &serde_json::Map<String, serde_json::Value>, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|k| get_f64(o, k))
}

// With -G1, ExifTool prefixes tags by IFD group: ExifIFD:, GPS:, QuickTime:, XMP-exif: ...
const DATE_ORIG_KEYS: [&str; 3] = [
    "ExifIFD:DateTimeOriginal",
    "EXIF:DateTimeOriginal",
    "Composite:DateTimeOriginal",
];
const OFFSET_KEYS: [&str; 3] = [
    "ExifIFD:OffsetTimeOriginal",
    "EXIF:OffsetTimeOriginal",
    "Composite:OffsetTimeOriginal",
];
const XMP_ORIG_KEYS: [&str; 2] = ["XMP-exif:DateTimeOriginal", "XMP:DateTimeOriginal"];
const CREATE_KEYS: [&str; 5] = [
    "XMP-xmp:CreateDate",
    "XMP:CreateDate",
    "XMP-xmp:ModifyDate",
    "XMP:ModifyDate",
    "IPTC:DateCreated",
];
const QT_KEYS: [&str; 4] = [
    "QuickTime:CreateDate",
    "QuickTime:MediaCreateDate",
    "Track:CreateDate",
    "TrackHeader:CreateDate",
];
const DIGITIZED_KEYS: [&str; 2] = ["ExifIFD:DateTimeDigitized", "ExifIFD:DateTime"];
const LAT_KEYS: [&str; 5] = [
    "GPS:GPSLatitude",
    "ExifIFD:GPSLatitude",
    "XMP-exif:GPSLatitude",
    "XMP:GPSLatitude",
    "Composite:GPSLatitude",
];
const LON_KEYS: [&str; 5] = [
    "GPS:GPSLongitude",
    "ExifIFD:GPSLongitude",
    "XMP-exif:GPSLongitude",
    "XMP:GPSLongitude",
    "Composite:GPSLongitude",
];
const ALT_KEYS: [&str; 2] = ["GPS:GPSAltitude", "ExifIFD:GPSAltitude"];
const CITY_KEYS: [&str; 5] = [
    "IPTC:City",
    "XMP-iptcExt:City",
    "XMP:city",
    "XMP-photoshop:City",
    "IPTC:LocationCreatedCityName",
];
const W_KEYS: [&str; 3] = [
    "File:ImageWidth",
    "ExifIFD:ImageWidth",
    "JpegIFD:ImageWidth",
];
const H_KEYS: [&str; 3] = [
    "File:ImageHeight",
    "ExifIFD:ImageHeight",
    "JpegIFD:ImageHeight",
];

/// Map an ExifTool JSON object to normalized metadata (spec §6, §7 priorities).
pub fn exiftool_to_metadata(o: &serde_json::Map<String, serde_json::Value>) -> MediaMetadata {
    let mut m = MediaMetadata::default();

    // --- date with strict priority ---
    let offset = get_any_str(o, &OFFSET_KEYS);
    if let Some(raw) = get_any_str(o, &DATE_ORIG_KEYS) {
        m.captured = Some(captured_date::from_exif(raw, offset));
    } else if let Some(raw) = get_any_str(o, &XMP_ORIG_KEYS) {
        m.captured = Some(captured_date::from_exif(raw, offset));
        if let Some(c) = m.captured.as_mut() {
            c.date_source = Some(crate::domain::captured_date::DateSource::Xmp);
        }
    } else if let Some(raw) = get_any_str(o, &CREATE_KEYS) {
        m.captured = Some(captured_date::from_exif(raw, None));
        if let Some(c) = m.captured.as_mut() {
            c.date_source = Some(crate::domain::captured_date::DateSource::Xmp);
        }
    } else if let Some(raw) = get_any_str(o, &QT_KEYS) {
        m.captured = Some(captured_date::from_container(raw));
    } else if let Some(raw) = get_any_str(o, &DIGITIZED_KEYS) {
        m.captured = Some(captured_date::from_exif(raw, None));
        if let Some(c) = m.captured.as_mut() {
            c.normalization_note = Some("Використано вторинне EXIF-поле дати".into());
        }
    }

    // --- GPS ---
    let lat = get_any_f64(o, &LAT_KEYS);
    let lon = get_any_f64(o, &LON_KEYS);
    if let (Some(la), Some(lo)) = (lat, lon) {
        let alt = get_any_f64(o, &ALT_KEYS);
        let p = GeoPoint {
            latitude: la,
            longitude: lo,
            altitude: alt,
        };
        if p.valid() {
            m.geo = Some(p);
        }
    }

    // --- explicit textual location fields ---
    m.place_text = get_any_str(o, &CITY_KEYS)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    // --- dimensions / codecs ---
    if let (Some(w), Some(h)) = (get_any_f64(o, &W_KEYS), get_any_f64(o, &H_KEYS)) {
        if w > 0.0 && h > 0.0 {
            m.width = Some(w as u32);
            m.height = Some(h as u32);
        }
    }
    m.mime = get_str(o, "MIMEType").map(|s| s.to_string());
    m
}

// ---------------------------------------------------------------------------
// FFprobe adapter (video containers)
// ---------------------------------------------------------------------------

pub struct Ffprobe {
    pub bin: Option<String>,
}

impl Ffprobe {
    pub fn new() -> Self {
        Self {
            bin: locate("ffprobe"),
        }
    }
}

impl Default for Ffprobe {
    fn default() -> Self {
        Self::new()
    }
}

impl MetadataReader for Ffprobe {
    fn name(&self) -> &'static str {
        "ffprobe"
    }
    fn is_available(&self) -> bool {
        self.bin.is_some()
    }
    fn read(&self, path: &Path) -> KResult<Option<MediaMetadata>> {
        let bin = self.bin.as_ref().ok_or_else(|| KadrError::Helper {
            tool: "FFprobe".into(),
            reason: "не знайдено у PATH".into(),
        })?;
        let mut cmd = Command::new(bin);
        no_window(&mut cmd);
        cmd.args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path);
        let wait =
            run_captured(&mut cmd, Duration::from_secs(60)).map_err(|e| KadrError::Helper {
                tool: "FFprobe".into(),
                reason: format!("не вдалося запустити: {e}"),
            })?;
        match wait {
            (Some(status), stdout, _) if status.success() => {
                parse_ffprobe_json(&String::from_utf8_lossy(&stdout)).map(Some)
            }
            (Some(_), _, stderr) => Err(KadrError::user_detail(
                "FFprobe не зміг прочитати файл",
                String::from_utf8_lossy(&stderr)
                    .chars()
                    .take(300)
                    .collect::<String>(),
            )),
            (None, _, _) => Err(KadrError::Helper {
                tool: "FFprobe".into(),
                reason: "таймаут виклику".into(),
            }),
        }
    }
}

pub fn parse_ffprobe_json(text: &str) -> KResult<MediaMetadata> {
    let v: serde_json::Value = serde_json::from_str(text)?;
    let mut m = MediaMetadata::default();
    if let Some(fmt) = v.get("format") {
        if let Some(dur) = fmt
            .get("duration")
            .and_then(|d| d.as_str())
            .and_then(|s| s.parse::<f64>().ok())
        {
            if dur.is_finite() && dur > 0.0 {
                m.duration_ms = Some((dur * 1000.0) as i64);
            }
        }
        if let Some(ct) = fmt
            .get("tags")
            .and_then(|t| t.get("creation_time"))
            .and_then(|c| c.as_str())
        {
            m.captured = Some(captured_date::from_container(ct));
        }
    }
    if let Some(streams) = v.get("streams").and_then(|s| s.as_array()) {
        for s in streams {
            let codec_type = s.get("codec_type").and_then(|c| c.as_str()).unwrap_or("");
            let codec = s
                .get("codec_name")
                .and_then(|c| c.as_str())
                .map(|c| c.to_string());
            if codec_type == "video" && m.video_codec.is_none() {
                m.video_codec = codec.clone();
                if let Some(w) = s.get("width").and_then(|w| w.as_i64()) {
                    m.width = Some(w as u32);
                }
                if let Some(h) = s.get("height").and_then(|h| h.as_i64()) {
                    m.height = Some(h as u32);
                }
            }
            if codec_type == "audio" && m.audio_codec.is_none() {
                m.audio_codec = codec;
            }
        }
    }
    // creation_time in tags may have appeared without format branch
    if m.captured.is_none() {
        if let Some(ct) = v
            .get("format")
            .and_then(|f| f.get("tags"))
            .and_then(|t| t.get("creation_time"))
            .and_then(|c| c.as_str())
        {
            m.captured = Some(captured_date::from_container(ct));
        }
    }
    Ok(m)
}

// ---------------------------------------------------------------------------
// Native image fallback (dimensions only — honest about what it can read)
// ---------------------------------------------------------------------------

pub struct NativeImage;

/// Built-in EXIF reader (kamadak-exif): JPEG, TIFF-based RAW, HEIF/AVIF and
/// PNG get dates + GPS even when ExifTool is missing or fails on a file.
/// Same tag priority as exiftool_to_metadata, just a narrower tag set.
impl MetadataReader for NativeImage {
    fn name(&self) -> &'static str {
        "native-exif"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn read(&self, path: &Path) -> KResult<Option<MediaMetadata>> {
        let mut m = MediaMetadata::default();

        if let Ok(file) = std::fs::File::open(path) {
            let mut br = std::io::BufReader::new(file);
            if let Ok(ex) = exif::Reader::new().read_from_container(&mut br) {
                use exif::{In, Tag, Value};
                let ascii = |t: Tag| -> Option<String> {
                    let f = ex.get_field(t, In::PRIMARY)?;
                    match &f.value {
                        Value::Ascii(parts) => parts
                            .first()
                            .map(|b| String::from_utf8_lossy(b).trim().to_owned())
                            .filter(|s| !s.is_empty()),
                        _ => None,
                    }
                };
                let dms = |t: Tag, rt: Tag, neg: &[u8]| -> Option<f64> {
                    let Value::Rational(parts) = &ex.get_field(t, In::PRIMARY)?.value else {
                        return None;
                    };
                    let deg = parts.first()?.to_f64()
                        + parts.get(1).map(|r| r.to_f64() / 60.0).unwrap_or(0.0)
                        + parts.get(2).map(|r| r.to_f64() / 3600.0).unwrap_or(0.0);
                    let neg = ex
                        .get_field(rt, In::PRIMARY)
                        .and_then(|f| match &f.value {
                            Value::Ascii(p) => p.first().and_then(|v| v.first()).copied(),
                            _ => None,
                        })
                        .map(|b| neg.contains(&b.to_ascii_lowercase()))
                        .unwrap_or(false);
                    Some(if neg { -deg } else { deg })
                };

                if let Some(raw) = ascii(Tag::DateTimeOriginal) {
                    let off = ascii(Tag::OffsetTimeOriginal).or_else(|| ascii(Tag::OffsetTime));
                    m.captured = Some(captured_date::from_exif(&raw, off.as_deref()));
                } else if let Some(raw) = ascii(Tag::DateTimeDigitized) {
                    let mut cd = captured_date::from_exif(&raw, None);
                    cd.normalization_note =
                        Some("Використано вторинне EXIF-поле дати (native)".into());
                    m.captured = Some(cd);
                }

                if let (Some(la), Some(lo)) = (
                    dms(Tag::GPSLatitude, Tag::GPSLatitudeRef, b"Ss"),
                    dms(Tag::GPSLongitude, Tag::GPSLongitudeRef, b"Ww"),
                ) {
                    let alt =
                        ex.get_field(Tag::GPSAltitude, In::PRIMARY)
                            .and_then(|f| match &f.value {
                                Value::Rational(v) => v.first().map(|r| r.to_f64()),
                                _ => None,
                            });
                    let p = GeoPoint {
                        latitude: la,
                        longitude: lo,
                        altitude: alt,
                    };
                    if p.valid() {
                        m.geo = Some(p);
                    }
                }
            }
        }

        if m.width.is_none() {
            let reader = image::ImageReader::open(path)
                .map_err(|e| {
                    KadrError::user_detail("Не вдалося відкрити зображення", e.to_string())
                })?
                .with_guessed_format()
                .map_err(|e| {
                    KadrError::user_detail("Не вдалося визначити формат", e.to_string())
                })?;
            if let Ok((w, h)) = reader.into_dimensions() {
                m.width = Some(w);
                m.height = Some(h);
            }
        }

        Ok(
            if m.captured.is_some() || m.geo.is_some() || m.width.is_some() {
                Some(m)
            } else {
                None
            },
        )
    }
}

/// Merge provider results by priority: exiftool first (EXIF wins), then
/// ffprobe fills the gaps, native reader last. Never overwrites a Confirmed
/// date with a less trustworthy one.
pub fn merge_metadata(
    primary: Option<MediaMetadata>,
    secondary: Option<MediaMetadata>,
    tertiary: Option<MediaMetadata>,
) -> MediaMetadata {
    fn pick<T>(a: Option<T>, b: Option<T>) -> Option<T> {
        a.or(b)
    }
    let a = primary.unwrap_or_default();
    let b = secondary.unwrap_or_default();
    let c = tertiary.unwrap_or_default();
    MediaMetadata {
        captured: a.captured.or(b.captured).or(c.captured),
        geo: pick(a.geo, b.geo),
        place_text: pick(a.place_text, b.place_text),
        width: pick(a.width, pick(b.width, c.width)),
        height: pick(a.height, pick(b.height, c.height)),
        duration_ms: pick(a.duration_ms, b.duration_ms),
        video_codec: pick(a.video_codec, b.video_codec),
        audio_codec: pick(a.audio_codec, b.audio_codec),
        mime: pick(a.mime, b.mime),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::captured_date::{DateSource, DateStatus};
    use chrono::Datelike;

    const JPG_JSON: &str = r#"[{
      "SourceFile": "/x/img.jpg",
      "ExifIFD:DateTimeOriginal": "2025:08:12 14:03:22",
      "ExifIFD:OffsetTimeOriginal": "+03:00",
      "GPS:GPSLatitude": 48.537878,
      "GPS:GPSLongitude": 37.92,
      "MIMEType": "image/jpeg",
      "File:ImageWidth": 4000,
      "File:ImageHeight": 3000
    }]"#;

    #[test]
    fn jpg_fixture_parses_date_gps() {
        let objs = parse_exiftool_json(JPG_JSON).unwrap();
        let m = exiftool_to_metadata(&objs[0]);
        let c = m.captured.clone().unwrap();
        assert_eq!(c.date_status, DateStatus::Confirmed);
        assert_eq!(c.date_source, Some(DateSource::Exif));
        assert_eq!(c.captured_at_local.unwrap().year(), 2025);
        assert_eq!(c.tz_offset_min, Some(180));
        let g = m.geo.unwrap();
        assert!((g.latitude - 48.537878).abs() < 1e-9);
        assert_eq!(m.width, Some(4000));
        assert_eq!(m.mime.as_deref(), Some("image/jpeg"));
    }

    const ZERO_DATE_JSON: &str = r#"[{
      "SourceFile": "/y/a.jpg",
      "ExifIFD:DateTimeOriginal": "0000:00:00 00:00:00"
    }]"#;

    #[test]
    fn zero_date_marked_needs_review() {
        let objs = parse_exiftool_json(ZERO_DATE_JSON).unwrap();
        let m = exiftool_to_metadata(&objs[0]);
        assert_eq!(m.captured.unwrap().date_status, DateStatus::NeedsReview);
    }

    const MOV_JSON: &str = r#"[{
      "SourceFile": "/z/vid.mov",
      "QuickTime:CreateDate": "2024:02:29 21:10:05",
      "MIMEType": "video/quicktime"
    }]"#;

    #[test]
    fn mov_container_date() {
        let objs = parse_exiftool_json(MOV_JSON).unwrap();
        let m = exiftool_to_metadata(&objs[0]);
        let c = m.captured.unwrap();
        assert_eq!(c.date_source, Some(DateSource::MediaContainer));
        assert_eq!(c.date_status, DateStatus::Confirmed);
        assert_eq!(c.captured_at_local.unwrap().month(), 2);
    }

    const GPS_ZERO_JSON: &str = r#"[{
      "SourceFile": "/z/junk.jpg",
      "GPS:GPSLatitude": 0, "GPS:GPSLongitude": 0
    }]"#;

    #[test]
    fn junk_gps_rejected() {
        let objs = parse_exiftool_json(GPS_ZERO_JSON).unwrap();
        let m = exiftool_to_metadata(&objs[0]);
        assert!(m.geo.is_none());
    }

    #[test]
    fn ffprobe_fixture() {
        let m = parse_ffprobe_json(
            r#"{
          "streams": [
            {"index":0,"codec_name":"hevc","codec_type":"video","width":1920,"height":1080},
            {"index":1,"codec_name":"aac","codec_type":"audio"}
          ],
          "format": {"duration":"12.5","tags":{"creation_time":"2024-05-01T10:00:00Z"}}
        }"#,
        )
        .unwrap();
        assert_eq!(m.duration_ms, Some(12500));
        assert_eq!(m.video_codec.as_deref(), Some("hevc"));
        assert_eq!(m.audio_codec.as_deref(), Some("aac"));
        assert_eq!(m.width, Some(1920));
        let c = m.captured.unwrap();
        assert_eq!(c.date_source, Some(DateSource::MediaContainer));
    }

    #[test]
    fn merge_keeps_confirmed_exif() {
        let a = MediaMetadata {
            captured: Some(captured_date::from_exif("2025:01:01 10:00:00", None)),
            ..Default::default()
        };
        let b = MediaMetadata {
            captured: Some(captured_date::from_container("2020-01-01T00:00:00Z")),
            duration_ms: Some(500),
            ..Default::default()
        };
        let m = merge_metadata(Some(a), Some(b), None);
        assert_eq!(m.captured.unwrap().date_source, Some(DateSource::Exif));
        assert_eq!(m.duration_ms, Some(500));
    }

    #[test]
    fn exiftool_roundtrip_date_gps() {
        // §18.3/§18.4: stamp a real JPEG through the actual helper and read it back.
        // Skips honestly when the helper is not installed.
        let et = ExifTool::new();
        let Some(bin) = et.bin.clone() else { return };
        let dir = std::env::temp_dir().join(format!("kadr-xrt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let jpg = dir.join("t.jpg");
        image::DynamicImage::new_rgb8(2, 2).save(&jpg).unwrap();
        let out = Command::new(&bin)
            .args([
                "-overwrite_original",
                "-DateTimeOriginal=2025:08:15 14:03:22",
                "-OffsetTimeOriginal=+03:00",
                "-GPSLatitude=48.853",
                "-GPSLatitudeRef=N",
                "-GPSLongitude=2.349",
                "-GPSLongitudeRef=E",
                jpg.to_str().unwrap(),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "exiftool: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let md = et.read(&jpg).unwrap().expect("metadata read");
        let cap = md.captured.expect("captured date");
        assert_eq!(cap.date_source, Some(captured_date::DateSource::Exif));
        assert_eq!(cap.date_status, captured_date::DateStatus::Confirmed);
        assert_eq!(
            cap.captured_at_local,
            Some(
                chrono::NaiveDateTime::parse_from_str("2025-08-15T14:03:22", "%Y-%m-%dT%H:%M:%S")
                    .unwrap()
            )
        );
        assert_eq!(cap.tz_offset_min, Some(180));
        assert_eq!(cap.captured_at_raw.as_deref(), Some("2025:08:15 14:03:22"));
        let geo = md.geo.expect("gps coords");
        assert!((geo.latitude - 48.853).abs() < 1e-3, "lat {}", geo.latitude);
        assert!(
            (geo.longitude - 2.349).abs() < 1e-3,
            "lon {}",
            geo.longitude
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn bundled_helper_resolved_before_path() {
        let dir = std::env::temp_dir().join(format!("kadr-hlp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("kadr-fake-tool");
        std::fs::write(&fake, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::env::set_var("KADR_HELPER_DIR", &dir);
        let found = locate("kadr-fake-tool").expect("bundled helper must be found");
        assert_eq!(found, fake.to_string_lossy());
        // tools not present anywhere resolve to None, never a panic
        assert!(locate("kadr-definitely-not-a-tool-xyz").is_none());
        std::env::remove_var("KADR_HELPER_DIR");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn explicit_override_beats_everything() {
        let dir = std::env::temp_dir().join(format!("kadr-ovr-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("mytool.exe");
        std::fs::write(&target, "x").unwrap();
        std::env::set_var("KADR_KADROVRTOOL", &target);
        assert_eq!(
            locate("kadrovrtool").as_deref(),
            Some(target.to_string_lossy().as_ref())
        );
        std::env::remove_var("KADR_KADROVRTOOL");
        std::fs::remove_dir_all(&dir).ok();
    }
}
