//! Directory scanner with incremental indexing, cancellation and per-file
//! error tolerance (TECHNICAL_SPEC §9).

use crate::domain::captured_date::{self, CapturedDate, DateSource, DateStatus};
use crate::domain::location::{LocationSource, PlaceStatus};
use crate::domain::media_item::{
    is_supported_media, kind_from_extension, MediaItem, MediaKind, MetadataStatus,
};
use crate::domain::operation::LogLevel;
use crate::errors::{KResult, KadrError};
use crate::persistence::db::{self, SourceRoot};
use crate::services::metadata::{self as meta, MediaMetadata, MetadataReader};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use walkdir::WalkDir;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScanOptions {
    pub follow_symlinks: bool,
    pub include_hidden: bool,
    /// Use filesystem mtime as a date fallback (spec: opt-in only).
    pub fs_date_fallback: bool,
    /// Hashing every file on first scan maxes CPU/disk for archives of tens
    /// of thousands of photos; the duplicates search is opt-in instead.
    pub compute_hashes: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScanProgress {
    pub found: i64,
    pub processed: i64,
    pub skipped: i64,
    pub errors: i64,
    pub current: String,
}

pub fn sha256_file(path: &Path) -> KResult<String> {
    use sha2::{Digest, Sha256};
    let mut f = fs::File::open(path).map_err(|e| KadrError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(|e| KadrError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
    }
}

pub fn normalize_path(p: &Path) -> String {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let s = abs.to_string_lossy().to_string();
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s
    }
}

fn mtime_secs(md: &std::fs::Metadata) -> i64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        md.mtime()
    }
    #[cfg(not(unix))]
    {
        md.modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}

fn file_identity(md: &fs::Metadata) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(format!("{}:{}", md.dev(), md.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = md;
        None
    }
}

struct CachedRow {
    size_bytes: i64,
    mtime: i64,
    sha256: Option<String>,
    metadata_status: String,
    captured_raw: Option<String>,
    item_id: String,
}

fn load_cached_row(conn: &Connection, normalized: &str) -> Option<CachedRow> {
    conn.query_row(
        "SELECT size_bytes, mtime, sha256, metadata_status, captured_at_raw, id FROM media_item WHERE normalized_path = ?1",
        rusqlite::params![normalized],
        |r| Ok(CachedRow {
            size_bytes: r.get(0)?,
            mtime: r.get(1)?,
            sha256: r.get(2)?,
            metadata_status: r.get(3)?,
            captured_raw: r.get(4)?,
            item_id: r.get(5)?,
        }),
    )
    .ok()
}

/// Merge normalized metadata into a MediaItem honoring sidecar/kind and
/// the fs-date-fallback policy.
fn fill_item_from_meta(item: &mut MediaItem, m: &MediaMetadata, fs_date_fallback: bool) {
    if let Some(c) = &m.captured {
        item.captured_at_raw = c.captured_at_raw.clone();
        item.captured_at_local = c
            .captured_at_local
            .map(|v| v.format("%Y-%m-%dT%H:%M:%S").to_string());
        item.captured_at_utc = c.captured_at_utc.map(|v| v.to_rfc3339());
        item.timezone_offset = c.tz_offset_min;
        item.date_source = c.date_source;
        item.date_status = c.date_status;
        item.normalization_note = c.normalization_note.clone();
    }
    if let Some(g) = &m.geo {
        item.latitude = Some(g.latitude);
        item.longitude = Some(g.longitude);
        item.altitude = g.altitude;
        if let Some(t) = &m.place_text {
            item.place_label = Some(t.clone());
            item.location_source = Some(LocationSource::Metadata);
            item.place_status = PlaceStatus::Resolved;
        } else if let Some(label) = super::geocode::resolve(g.latitude, g.longitude) {
            item.place_label = Some(label);
            item.location_source = Some(LocationSource::Geocoder);
            item.place_status = PlaceStatus::Resolved;
        } else {
            item.place_status = PlaceStatus::CoordsOnly;
            item.location_source = Some(LocationSource::None);
        }
    } else if let Some(t) = &m.place_text {
        item.place_label = Some(t.clone());
        item.location_source = Some(LocationSource::Metadata);
        item.place_status = PlaceStatus::Resolved;
    } else {
        item.place_status = PlaceStatus::NoGeo;
    }
    item.width = m.width.or(item.width);
    item.height = m.height.or(item.height);
    item.duration_ms = m.duration_ms.or(item.duration_ms);
    item.video_codec = m.video_codec.clone().or(item.video_codec.clone());
    item.audio_codec = m.audio_codec.clone().or(item.audio_codec.clone());
    if item.mime_or_detected_type.is_none() {
        item.mime_or_detected_type = m.mime.clone();
    }
    if item.date_status == DateStatus::Unknown && fs_date_fallback {
        let c: CapturedDate = captured_date::from_filesystem(item.mtime);
        item.captured_at_raw = c.captured_at_raw;
        item.captured_at_local = c
            .captured_at_local
            .map(|v| v.format("%Y-%m-%dT%H:%M:%S").to_string());
        item.captured_at_utc = c.captured_at_utc.map(|v| v.to_rfc3339());
        item.timezone_offset = c.tz_offset_min;
        item.date_source = Some(DateSource::FileSystem);
        item.date_status = c.date_status;
        item.normalization_note = c.normalization_note;
    }
}

/// Recursively scan a source root. Writes index rows incrementally in
/// transactions; never aborts the whole tree on a single file error.
pub fn scan_root(
    conn: &Connection,
    root: &SourceRoot,
    opts: &ScanOptions,
    job_id: &str,
    cancel: &AtomicBool,
    on_progress: &dyn Fn(&ScanProgress),
) -> KResult<ScanProgress> {
    let root_path = Path::new(&root.path);
    if !root_path.is_dir() {
        return Err(KadrError::user(format!(
            "Папка «{}» недоступна. Перевірте шлях або підключення диска.",
            root.path
        )));
    }
    let started = db::now_ms();
    db::create_scan_job(conn, job_id, &root.id)?;

    let exif = meta::ExifTool::new();
    let ff = meta::Ffprobe::new();

    let mut progress = ScanProgress::default();
    let mut pending: Vec<(PathBuf, MediaItem)> = Vec::new();

    let mut tx = conn.unchecked_transaction()?;

    let walker = WalkDir::new(root_path)
        .follow_links(opts.follow_symlinks)
        .max_depth(64)
        .into_iter()
        .filter_entry(|e| {
            if !opts.include_hidden && e.depth() > 0 {
                if let Some(name) = e.file_name().to_str() {
                    if name.starts_with('.')
                        || name == "node_modules"
                        || name.starts_with("$RECYCLE")
                    {
                        return false;
                    }
                }
            }
            true
        });

    let mut last_emit = std::time::Instant::now();
    for entry in walker {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                progress.errors += 1;
                db::log_event(
                    conn,
                    job_id,
                    LogLevel::Warn,
                    "scan",
                    &format!(
                        "Пропущено директорію: {}",
                        err.to_string().chars().take(200).collect::<String>()
                    ),
                )?;
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        if entry.file_type().is_symlink() && !opts.follow_symlinks {
            progress.skipped += 1;
            continue;
        }
        let path = entry.path();
        let md = match entry.metadata() {
            Ok(m) => m,
            Err(_) => {
                progress.errors += 1;
                continue;
            }
        };
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let kind = kind_from_extension(&ext);
        progress.found += 1;

        if kind == MediaKind::Other {
            progress.skipped += 1;
            db::log_event(
                conn,
                job_id,
                LogLevel::Info,
                "scan",
                &format!(
                    "Непідтримуваний формат: {}",
                    path.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default()
                ),
            )?;
            continue;
        }

        let normalized = normalize_path(path);
        let cached = load_cached_row(conn, &normalized);
        let fingerprint_fresh = cached
            .as_ref()
            .map(|c| c.size_bytes == md.len() as i64 && c.mtime == mtime_secs(&md))
            .unwrap_or(false);

        let mut item = MediaItem {
            id: uuid::Uuid::new_v4().to_string(),
            source_root_id: root.id.clone(),
            path: path.to_string_lossy().to_string(),
            normalized_path: normalized.clone(),
            file_name: path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            extension: ext.clone(),
            media_kind: kind,
            size_bytes: md.len() as i64,
            mtime: mtime_secs(&md),
            file_identity: file_identity(&md),
            sha256: cached.as_ref().and_then(|c| c.sha256.clone()),
            fingerprint_updated_at: Some(db::now_ms()),
            mime_or_detected_type: None,
            metadata_status: MetadataStatus::Unavailable,
            thumbnail_path: None,
            captured_at_raw: None,
            captured_at_local: None,
            captured_at_utc: None,
            timezone_offset: None,
            date_source: None,
            date_status: DateStatus::Unknown,
            normalization_note: None,
            latitude: None,
            longitude: None,
            altitude: None,
            place_label: None,
            location_source: None,
            place_status: PlaceStatus::NoGeo,
            manual_place_override: false,
            duration_ms: None,
            width: None,
            height: None,
            video_codec: None,
            audio_codec: None,
            scan_timestamp: started,
            last_error: None,
            is_sidecar: kind == MediaKind::Sidecar,
            sidecar_parent_id: None,
        };

        // Keep the stable row id for overrides bookkeeping
        if let Some(c) = &cached {
            item.id = c.item_id.clone();
        }

        // Hash policy: skip re-hash when size+mtime unchanged and hash cached
        if opts.compute_hashes && !(fingerprint_fresh && item.sha256.is_some()) {
            match sha256_file(path) {
                Ok(h) => item.sha256 = Some(h),
                Err(e) => {
                    progress.errors += 1;
                    item.last_error = Some(e.friendly());
                }
            }
        }

        let meta_ok_cached = fingerprint_fresh
            && cached
                .as_ref()
                .map(|c| c.metadata_status == "ok" && c.captured_raw.is_some())
                .unwrap_or(false);

        if !meta_ok_cached {
            pending.push((path.to_path_buf(), item));
        } else {
            // metadata still fresh: keep previous values by re-reading the row
            if let Some(existing) = db::get_media_item(conn, &item.id)? {
                let mut merged = item;
                merged.captured_at_raw = existing.captured_at_raw;
                merged.captured_at_local = existing.captured_at_local;
                merged.captured_at_utc = existing.captured_at_utc;
                merged.timezone_offset = existing.timezone_offset;
                merged.date_source = existing.date_source;
                merged.date_status = existing.date_status;
                merged.normalization_note = existing.normalization_note;
                merged.latitude = existing.latitude;
                merged.longitude = existing.longitude;
                merged.altitude = existing.altitude;
                merged.place_label = existing.place_label;
                merged.location_source = existing.location_source;
                merged.place_status = existing.place_status;
                merged.width = existing.width;
                merged.height = existing.height;
                merged.duration_ms = existing.duration_ms;
                merged.video_codec = existing.video_codec;
                merged.audio_codec = existing.audio_codec;
                merged.mime_or_detected_type = existing.mime_or_detected_type;
                merged.thumbnail_path = existing.thumbnail_path;
                merged.metadata_status = existing.metadata_status;
                db::upsert_media_item(&tx, &merged)?;
            } else {
                db::upsert_media_item(&tx, &item)?;
            }
        }

        if meta_ok_cached {
            progress.processed += 1;
        }
        // Metadata is collected in batches DURING the walk, so the progress
        // bar means "fully processed", not merely "discovered".
        if pending.len() >= FLUSH_BATCH {
            let chunk = std::mem::take(&mut pending);
            flush_batch(
                conn,
                &mut tx,
                &exif,
                &ff,
                opts,
                job_id,
                &chunk,
                &mut progress,
            )?;
            tx.commit()?;
            tx = conn.unchecked_transaction()?;
            db::update_scan_progress(
                conn,
                job_id,
                progress.found,
                progress.processed,
                progress.skipped,
                progress.errors,
                &progress.current,
            )?;
            on_progress(&progress);
            last_emit = std::time::Instant::now();
        } else if last_emit.elapsed() >= std::time::Duration::from_secs(1) {
            // Heartbeat even on slow media (network drives): never look frozen.
            progress.current = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if progress.processed % 500 == 0 {
                tx.commit()?;
                tx = conn.unchecked_transaction()?;
            }
            db::update_scan_progress(
                conn,
                job_id,
                progress.found,
                progress.processed,
                progress.skipped,
                progress.errors,
                &progress.current,
            )?;
            on_progress(&progress);
            last_emit = std::time::Instant::now();
        }
    }

    // Flush the metadata batches left over when the walk finished.
    while !pending.is_empty() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let take = FLUSH_BATCH.min(pending.len());
        let chunk: Vec<(PathBuf, MediaItem)> = pending.drain(..take).collect();
        flush_batch(
            conn,
            &mut tx,
            &exif,
            &ff,
            opts,
            job_id,
            &chunk,
            &mut progress,
        )?;
        tx.commit()?;
        tx = conn.unchecked_transaction()?;
        db::update_scan_progress(
            conn,
            job_id,
            progress.found,
            progress.processed,
            progress.skipped,
            progress.errors,
            &progress.current,
        )?;
        on_progress(&progress);
    }

    tx.commit()?;
    let cancelled = cancel.load(Ordering::Relaxed);
    if !cancelled {
        let stale = db::delete_missing_under(conn, &root.id, started)? as i64;
        if stale > 0 {
            db::log_event(
                conn,
                job_id,
                LogLevel::Info,
                "scan",
                &format!("Видалено з індексу {} відсутніх файлів", stale),
            )?;
        }
        link_sidecars(conn)?;
    }
    db::finish_scan_job(conn, job_id, cancelled)?;
    on_progress(&progress);
    Ok(progress)
}

/// Cheap counting pass over supported media — lets the UI draw a determinate
/// progress bar with an ETA before the expensive metadata phase starts.
pub fn count_media(root: &Path, opts: &ScanOptions) -> i64 {
    WalkDir::new(root)
        .follow_links(opts.follow_symlinks)
        .max_depth(64)
        .into_iter()
        .filter_entry(|e| {
            if !opts.include_hidden && e.depth() > 0 {
                if let Some(name) = e.file_name().to_str() {
                    if name.starts_with('.')
                        || name == "node_modules"
                        || name.starts_with("$RECYCLE")
                    {
                        return false;
                    }
                }
            }
            true
        })
        .filter_map(std::result::Result::ok)
        .filter(|e| {
            e.file_type().is_file()
                && is_supported_media(
                    e.path()
                        .extension()
                        .map(|x| x.to_string_lossy().to_lowercase())
                        .unwrap_or_default()
                        .as_str(),
                )
        })
        .count() as i64
}

/// Metadata batch size: big enough to amortize the ExifTool call, small
/// enough that progress ticks along smoothly.
const FLUSH_BATCH: usize = 200;

/// Read metadata for one batch of discovered files and persist the rows.
/// `progress.processed` advances here only — the bar counts fully processed
/// files, because users judge "done" by real work, not by discovery.
/// How many helper processes read one batch in parallel. Metadata reading is
/// process-spawn + file IO bound, so a few concurrent ExifTool runs multiply
/// throughput (learned from a parallel-worker implementation; Windows
/// defenders make serial spawns especially expensive).
fn meta_workers() -> usize {
    static W: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *W.get_or_init(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2)
            .clamp(1, 4)
    })
}

#[allow(clippy::too_many_arguments)] // one wiring struct for 3 call sites would be heavier
fn flush_batch(
    conn: &Connection,
    tx: &mut rusqlite::Transaction,
    exif: &meta::ExifTool,
    ff: &meta::Ffprobe,
    opts: &ScanOptions,
    job_id: &str,
    chunk: &[(PathBuf, MediaItem)],
    progress: &mut ScanProgress,
) -> KResult<()> {
    // Read metadata for all files in the batch using a few parallel helper
    // processes; persistence stays on this thread (single SQLite conn).
    let workers = meta_workers().min(chunk.len().div_ceil(25).max(1));
    let slice = chunk.len().div_ceil(workers);
    let mut merged: std::collections::HashMap<PathBuf, MediaMetadata> = std::thread::scope(|s| {
        let handles: Vec<_> = chunk
            .chunks(slice)
            .map(|sl| {
                let files: Vec<PathBuf> = sl.iter().map(|(p, _)| p.clone()).collect();
                let kinds: Vec<MediaKind> = sl.iter().map(|(_, i)| i.media_kind).collect();
                s.spawn(move || {
                    let mut out = Vec::with_capacity(files.len());
                    let mut errs: Vec<(PathBuf, String)> = Vec::new();
                    let results = exif.read_batch(&files);
                    let mut by_path: std::collections::HashMap<PathBuf, MediaMetadata> =
                        std::collections::HashMap::new();
                    for (p, r) in results {
                        match r {
                            Ok(m) => {
                                by_path.insert(p, m);
                            }
                            Err(e) => {
                                // video may still be readable through ffprobe; other errors are logged
                                errs.push((p.clone(), e.friendly().to_string()));
                            }
                        }
                    }
                    for (p, kind) in files.into_iter().zip(kinds) {
                        let mut m = by_path.remove(&p).unwrap_or_default();
                        let is_video = kind == MediaKind::Video;
                        if is_video {
                            if let Ok(Some(fm)) = ff.read(&p) {
                                // EXIF/QuickTime date from ExifTool wins; ffprobe fills codecs/duration
                                m = meta::merge_metadata(Some(m), Some(fm), None);
                            }
                        }
                        if !is_video
                            && (m.width.is_none() || m.captured.is_none() || m.geo.is_none())
                        {
                            if let Ok(Some(nm)) = meta::NativeImage.read(&p) {
                                m = meta::merge_metadata(Some(m), Some(nm), None);
                            }
                        }
                        out.push((p, m));
                    }
                    (out, errs)
                })
            })
            .collect();
        let mut acc = std::collections::HashMap::new();
        for h in handles {
            if let Ok((out, errs)) = h.join() {
                for (p, msg) in errs {
                    let _ = db::log_event(
                        conn,
                        job_id,
                        LogLevel::Warn,
                        "metadata",
                        &format!("{}: {}", p.display(), msg),
                    );
                }
                for (p, m) in out {
                    acc.insert(p, m);
                }
            }
        }
        acc
    });
    for (p, mut item) in chunk.iter().cloned() {
        let m = merged.remove(&p).unwrap_or_default();
        let had_any =
            m.captured.is_some() || m.geo.is_some() || m.width.is_some() || m.mime.is_some();
        item.metadata_status = if had_any {
            MetadataStatus::Ok
        } else if exif.bin.is_none() {
            MetadataStatus::Unavailable
        } else {
            MetadataStatus::Unsupported
        };
        if m.captured.is_none() {
            item.date_status = DateStatus::Unknown;
        }
        fill_item_from_meta(&mut item, &m, opts.fs_date_fallback);
        if item.date_status == DateStatus::NeedsReview {
            item.metadata_status = MetadataStatus::NeedsReview;
        }
        db::upsert_media_item(tx, &item)?;
        progress.processed += 1;
    }
    Ok(())
}

/// Apple live photos and edit sidecars: a same-stem sibling file travels with
/// the main item and is hidden from the gallery (spec §6, §8).
pub fn link_sidecars(conn: &Connection) -> KResult<()> {
    let pairs: Vec<(String, String, String, String)> = conn
        .prepare(
            "SELECT id, normalized_path, media_kind, extension FROM media_item
             WHERE media_kind IN ('sidecar','video','photo')",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<Result<_, _>>()?;

    let photos: std::collections::HashMap<String, String> = pairs
        .iter()
        .filter(|(_, _, k, _)| k == "photo")
        .map(|(_, np, _, _)| {
            let (dir, stem) = dir_stem(np);
            (format!("{dir}\u{1}{stem}"), np.clone())
        })
        .collect();

    let mut stmt = conn.prepare("UPDATE media_item SET media_kind='sidecar', is_sidecar=1, sidecar_parent_id=(SELECT id FROM media_item WHERE normalized_path=?2) WHERE id=?1")?;
    for (id, np, kind, ext) in &pairs {
        let (dir, stem) = dir_stem(np);
        let key = format!("{dir}\u{1}{stem}");
        let is_sidecar_ext = matches!(ext.as_str(), "aae" | "aa" | "xmp" | "mp");
        if kind == "sidecar" {
            if let Some(parent_np) = photos.get(&key) {
                stmt.execute(rusqlite::params![id, parent_np])?;
            } else {
                conn.execute(
                    "UPDATE media_item SET media_kind='sidecar', is_sidecar=1 WHERE id=?1",
                    rusqlite::params![id],
                )?;
            }
        } else if kind == "video" && is_sidecar_ext {
            // unreachable branch kept for clarity
        } else if kind == "video" {
            if let Some(parent_np) = photos.get(&key) {
                stmt.execute(rusqlite::params![id, parent_np])?;
            }
        }
    }
    Ok(())
}

fn dir_stem(normalized: &str) -> (String, String) {
    let p = Path::new(normalized);
    let dir = p
        .parent()
        .map(|d| d.to_string_lossy().to_string())
        .unwrap_or_default();
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    (dir, stem)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::db::{add_source_root, query_media, MediaFilter};
    use std::fs;

    fn touch_file(path: &Path, content: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn scan_index_rescan_idempotent_and_no_cycles() {
        let dir = std::env::temp_dir().join(format!("kadr-scan-{}", uuid::Uuid::new_v4()));
        let a = dir.join("photos/2025/vacation/IMG_1.jpg");
        touch_file(&a, b"hello");
        let b = dir.join("photos/sub/video.mov");
        touch_file(&b, b"movdata");
        let c = dir.join("photos/notes.txt");
        touch_file(&c, b"skip me");
        // symlink loop must not hang (follow disabled)
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&dir, dir.join("loop")).ok();
        }

        let conn = db::open_db(":memory:").unwrap();
        let root = add_source_root(&conn, dir.to_str().unwrap()).unwrap();
        let cancel = AtomicBool::new(false);
        let opts = ScanOptions {
            compute_hashes: true,
            ..Default::default()
        };
        let p1 = scan_root(&conn, &root, &opts, "job1", &cancel, &|_| {}).unwrap();
        let (items, total) = query_media(&conn, &MediaFilter::default()).unwrap();
        assert_eq!(total, 2, "jpg+mov indexed, txt skipped: {items:?}");

        let p2 = scan_root(&conn, &root, &opts, "job2", &cancel, &|_| {}).unwrap();
        let (_, total2) = query_media(&conn, &MediaFilter::default()).unwrap();
        assert_eq!(total2, 2, "rescan must not duplicate");
        assert!(p1.found >= 2 && p2.found >= 2);

        // sha256 present for dedup
        let (items, _) = query_media(
            &conn,
            &MediaFilter {
                search: Some("IMG".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(items[0].sha256.is_some());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scan_walks_every_branch_and_multiple_roots_are_indexed() {
        // Regression (Windows field report): a root with many sibling
        // folders must have every branch indexed, and each source root
        // scans independently with its own journal job.
        let dir = std::env::temp_dir().join(format!("kadr-wide-{}", uuid::Uuid::new_v4()));
        let dir2 = std::env::temp_dir().join(format!("kadr-wide2-{}", uuid::Uuid::new_v4()));
        let mut expected = 0usize;
        for year in ["2019", "2020", "2021", "2022", "2023"] {
            for sub in ["vacation", "family", "events", "misc"] {
                for n in 0..3 {
                    touch_file(&dir.join(format!("{year}/{sub}/photo_{n}.jpg")), b"x");
                    expected += 1;
                }
            }
        }
        // deep branch too
        touch_file(&dir.join("a/b/c/d/e/deep.jpg"), b"x");
        expected += 1;
        for n in 0..5 {
            touch_file(&dir2.join(format!("second_root_{n}.jpg")), b"x");
            expected += 1;
        }

        let conn = db::open_db(":memory:").unwrap();
        let root1 = add_source_root(&conn, dir.to_str().unwrap()).unwrap();
        let root2 = add_source_root(&conn, dir2.to_str().unwrap()).unwrap();
        let cancel = AtomicBool::new(false);
        let expected_roots = [61, 5];
        for (i, root) in [&root1, &root2].into_iter().enumerate() {
            let p = scan_root(
                &conn,
                root,
                &ScanOptions::default(),
                &format!("job{i}"),
                &cancel,
                &|_| {},
            )
            .unwrap();
            // processed must mean "fully processed (metadata included)", not "found"
            assert_eq!(p.processed, expected_roots[i]);
        }
        assert_eq!(count_media(&dir, &ScanOptions::default()), 61);
        let (_, total) = query_media(&conn, &MediaFilter::default()).unwrap();
        assert_eq!(
            total as usize, expected,
            "every branch of both roots indexed"
        );
        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&dir2).ok();
    }

    #[test]
    fn fs_fallback_policy() {
        let dir = std::env::temp_dir().join(format!("kadr-fs-{}", uuid::Uuid::new_v4()));
        let a = dir.join("a.jpg");
        touch_file(&a, b"x");
        let conn = db::open_db(":memory:").unwrap();
        let root = add_source_root(&conn, dir.to_str().unwrap()).unwrap();
        let cancel = AtomicBool::new(false);
        // fallback OFF: date unknown
        scan_root(
            &conn,
            &root,
            &ScanOptions {
                compute_hashes: false,
                ..Default::default()
            },
            "j1",
            &cancel,
            &|_| {},
        )
        .unwrap();
        let (items, _) = query_media(&conn, &MediaFilter::default()).unwrap();
        assert_eq!(items[0].date_status, DateStatus::Unknown);
        // fallback ON: filesystem date with Fallback status
        scan_root(
            &conn,
            &root,
            &ScanOptions {
                compute_hashes: false,
                fs_date_fallback: true,
                ..Default::default()
            },
            "j2",
            &cancel,
            &|_| {},
        )
        .unwrap();
        let (items, _) = query_media(&conn, &MediaFilter::default()).unwrap();
        assert_eq!(items[0].date_status, DateStatus::Fallback);
        assert_eq!(items[0].date_source, Some(DateSource::FileSystem));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_files_do_not_stop_the_scan() {
        // §18.11: one broken file must never abort the whole tree walk.
        let dir = std::env::temp_dir().join(format!("kadr-cor-{}", uuid::Uuid::new_v4()));
        touch_file(
            &dir.join("ok.jpg"),
            b"\xff\xd8\xff\xe0 plausible jpeg payload",
        );
        touch_file(&dir.join("broken.jpg"), b"\xff\xd8 then pure garbage");
        touch_file(
            &dir.join("nested/deeper/late.png"),
            b"\x89PNG\r\n\x1a\n truncated tail",
        );
        let conn = db::open_db(":memory:").unwrap();
        let root = add_source_root(&conn, dir.to_str().unwrap()).unwrap();
        let cancel = AtomicBool::new(false);
        let p = scan_root(
            &conn,
            &root,
            &ScanOptions::default(),
            "jobc",
            &cancel,
            &|_| {},
        )
        .unwrap();
        let (_, total) = query_media(&conn, &MediaFilter::default()).unwrap();
        assert_eq!(
            total, 3,
            "every file stays indexed even when metadata fails"
        );
        assert_eq!(p.processed, total);
        fs::remove_dir_all(&dir).ok();
    }

    /// Dev bench (ignored): scans a 3000-file EXIF tree, prints wall time.
    /// Run: cargo test --release scan_bench -- --ignored --nocapture
    #[test]
    #[ignore = "dev bench, needs /tmp/demo-photos EXIF files"]
    fn scan_bench_3000() {
        let demo = std::path::Path::new("/tmp/demo-photos");
        let Ok(samples) = std::fs::read_dir(demo) else {
            eprintln!("bench skipped: /tmp/demo-photos missing");
            return;
        };
        let samples: Vec<std::path::PathBuf> = samples
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().map(|e| e == "jpg").unwrap_or(false))
            .collect();
        assert!(!samples.is_empty());
        let dir = std::env::temp_dir().join(format!("kadr-bench-{}", uuid::Uuid::new_v4()));
        let mut n = 0;
        for f in 0..3000 {
            let d = dir.join(format!("y{}/m{:02}/s{}", f / 50, (f / 10) % 5, f % 10));
            fs::create_dir_all(&d).unwrap();
            let s = &samples[f % samples.len()];
            fs::copy(s, d.join(format!("photo_{f:04}.jpg"))).unwrap();
            n += 1;
        }
        let conn = db::open_db(":memory:").unwrap();
        let root = add_source_root(&conn, dir.to_str().unwrap()).unwrap();
        let cancel = AtomicBool::new(false);
        let opts = ScanOptions::default();
        let t0 = std::time::Instant::now();
        let p = scan_root(&conn, &root, &opts, "bench", &cancel, &|_| {}).unwrap();
        let dt = t0.elapsed();
        eprintln!(
            "BENCH: found={} processed={} in {:.2}s ({:.0} files/s)",
            p.found,
            p.processed,
            dt.as_secs_f64(),
            p.found as f64 / dt.as_secs_f64()
        );
        assert_eq!(p.found, n);
        std::fs::remove_dir_all(&dir).ok();
    }
}
