//! SQLite persistence: schema, migrations, CRUD (TECHNICAL_SPEC §14).

use crate::domain::captured_date::{DateSource, DateStatus};
use crate::domain::location::{LocationSource, PlaceStatus};
use crate::domain::media_item::{MediaItem, MediaKind, MetadataStatus};
use crate::domain::operation::{
    LogLevel, OperationLogEntry, OperationMode, PlanItem, PlanItemStatus, PlanWarning,
};
pub use crate::domain::operation::LogLevel as DbLogLevel;
use crate::errors::{KResult, KadrError};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: i64 = 1;

pub fn open_db(path: &str) -> KResult<Connection> {
    let conn = Connection::open(path)
        .map_err(|e| KadrError::user_detail("Не вдалося відкрити базу даних", e.to_string()))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&conn)?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> KResult<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version < 1 {
        conn.execute_batch(
            "BEGIN;
            CREATE TABLE IF NOT EXISTS source_root(
                id TEXT PRIMARY KEY,
                path TEXT NOT NULL UNIQUE,
                status TEXT NOT NULL DEFAULT 'ok',
                added_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS media_item(
                id TEXT PRIMARY KEY,
                source_root_id TEXT REFERENCES source_root(id) ON DELETE CASCADE,
                path TEXT NOT NULL,
                normalized_path TEXT NOT NULL UNIQUE,
                file_name TEXT NOT NULL,
                extension TEXT NOT NULL,
                media_kind TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                mtime INTEGER NOT NULL,
                file_identity TEXT,
                sha256 TEXT,
                fingerprint_updated_at INTEGER,
                mime_or_detected_type TEXT,
                metadata_status TEXT NOT NULL,
                thumbnail_path TEXT,
                captured_at_raw TEXT,
                captured_at_local TEXT,
                captured_at_utc TEXT,
                timezone_offset INTEGER,
                date_source TEXT,
                date_status TEXT NOT NULL,
                normalization_note TEXT,
                latitude REAL,
                longitude REAL,
                altitude REAL,
                place_label TEXT,
                location_source TEXT,
                place_status TEXT NOT NULL,
                manual_place_override INTEGER NOT NULL DEFAULT 0,
                duration_ms INTEGER,
                width INTEGER,
                height INTEGER,
                video_codec TEXT,
                audio_codec TEXT,
                scan_timestamp INTEGER NOT NULL,
                last_error TEXT,
                is_sidecar INTEGER NOT NULL DEFAULT 0,
                sidecar_parent_id TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_media_normalized ON media_item(normalized_path);
            CREATE INDEX IF NOT EXISTS idx_media_date ON media_item(captured_at_local);
            CREATE INDEX IF NOT EXISTS idx_media_place ON media_item(place_label);
            CREATE INDEX IF NOT EXISTS idx_media_kind ON media_item(media_kind);
            CREATE INDEX IF NOT EXISTS idx_media_sha ON media_item(sha256);
            CREATE INDEX IF NOT EXISTS idx_media_date_status ON media_item(date_status);
            CREATE INDEX IF NOT EXISTS idx_media_place_status ON media_item(place_status);
            CREATE INDEX IF NOT EXISTS idx_media_root ON media_item(source_root_id);

            CREATE TABLE IF NOT EXISTS scan_job(
                id TEXT PRIMARY KEY,
                root_id TEXT,
                state TEXT NOT NULL,
                found INTEGER NOT NULL DEFAULT 0,
                processed INTEGER NOT NULL DEFAULT 0,
                skipped INTEGER NOT NULL DEFAULT 0,
                errors INTEGER NOT NULL DEFAULT 0,
                started_at INTEGER NOT NULL,
                finished_at INTEGER,
                cancelled INTEGER NOT NULL DEFAULT 0,
                current_file TEXT
            );
            CREATE TABLE IF NOT EXISTS operation_plan(
                id TEXT PRIMARY KEY,
                created_at INTEGER NOT NULL,
                mode TEXT NOT NULL,
                destination_root TEXT NOT NULL,
                template TEXT NOT NULL,
                settings_json TEXT,
                state TEXT NOT NULL DEFAULT 'preview'
            );
            CREATE TABLE IF NOT EXISTS operation_item(
                id TEXT PRIMARY KEY,
                plan_id TEXT NOT NULL REFERENCES operation_plan(id) ON DELETE CASCADE,
                media_id TEXT NOT NULL,
                source_path TEXT NOT NULL,
                destination_path TEXT NOT NULL,
                mode TEXT NOT NULL,
                status TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                sha256_before TEXT,
                sha256_after TEXT,
                warnings_json TEXT,
                sidecar_of TEXT,
                error TEXT,
                started_at INTEGER,
                finished_at INTEGER,
                undo_source_path TEXT,
                undo_dest_path TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_opitem_plan ON operation_item(plan_id);
            CREATE INDEX IF NOT EXISTS idx_opitem_status ON operation_item(status);

            CREATE TABLE IF NOT EXISTS operation_log(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                job_id TEXT NOT NULL,
                level TEXT NOT NULL,
                action TEXT NOT NULL,
                message TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS app_settings(
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS geocode_cache(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                lat REAL NOT NULL,
                lon REAL NOT NULL,
                label TEXT NOT NULL,
                db_version TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_geocode ON geocode_cache(lat, lon, db_version);
            PRAGMA user_version = 1;
            COMMIT;",
        )?;
    }
    Ok(())
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

// ---------- source roots ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceRoot {
    pub id: String,
    pub path: String,
    pub status: String,
    pub added_at: i64,
}

pub fn add_source_root(conn: &Connection, path: &str) -> KResult<SourceRoot> {
    let root = SourceRoot {
        id: uuid::Uuid::new_v4().to_string(),
        path: path.to_string(),
        status: "ok".into(),
        added_at: now_ms(),
    };
    conn.execute(
        "INSERT INTO source_root(id, path, status, added_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(path) DO UPDATE SET status='ok'",
        params![root.id, root.path, root.status, root.added_at],
    )?;
    let existing: Option<String> = conn.query_row(
        "SELECT id FROM source_root WHERE path = ?1",
        params![root.path],
        |r| r.get(0),
    )?;
    if let Some(id) = existing {
        Ok(SourceRoot { id, ..root })
    } else {
        Ok(root)
    }
}

pub fn list_source_roots(conn: &Connection) -> KResult<Vec<SourceRoot>> {
    let mut stmt = conn.prepare("SELECT id, path, status, added_at FROM source_root ORDER BY added_at")?;
    let rows = stmt.query_map([], |r| {
        Ok(SourceRoot {
            id: r.get(0)?,
            path: r.get(1)?,
            status: r.get(2)?,
            added_at: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn remove_source_root(conn: &Connection, id: &str) -> KResult<()> {
    conn.execute("DELETE FROM source_root WHERE id = ?1", params![id])?;
    Ok(())
}

// ---------- media items ----------

pub const MEDIA_COLS: &str = "id, source_root_id, path, normalized_path, file_name, extension, media_kind, \
 size_bytes, mtime, file_identity, sha256, fingerprint_updated_at, mime_or_detected_type, metadata_status, thumbnail_path, \
 captured_at_raw, captured_at_local, captured_at_utc, timezone_offset, date_source, date_status, normalization_note, \
 latitude, longitude, altitude, place_label, location_source, place_status, manual_place_override, \
 duration_ms, width, height, video_codec, audio_codec, scan_timestamp, last_error, is_sidecar, sidecar_parent_id";

pub(crate) fn row_to_media(r: &rusqlite::Row) -> rusqlite::Result<MediaItem> {
    Ok(MediaItem {
        id: r.get(0)?,
        source_root_id: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
        path: r.get(2)?,
        normalized_path: r.get(3)?,
        file_name: r.get(4)?,
        extension: r.get(5)?,
        media_kind: parse_kind(&r.get::<_, String>(6)?),
        size_bytes: r.get(7)?,
        mtime: r.get(8)?,
        file_identity: r.get(9)?,
        sha256: r.get(10)?,
        fingerprint_updated_at: r.get(11)?,
        mime_or_detected_type: r.get(12)?,
        metadata_status: parse_meta_status(&r.get::<_, String>(13)?),
        thumbnail_path: r.get(14)?,
        captured_at_raw: r.get(15)?,
        captured_at_local: r.get(16)?,
        captured_at_utc: r.get(17)?,
        timezone_offset: r.get(18)?,
        date_source: r.get::<_, Option<String>>(19)?.as_deref().map(parse_date_source),
        date_status: parse_date_status(&r.get::<_, String>(20)?),
        normalization_note: r.get(21)?,
        latitude: r.get(22)?,
        longitude: r.get(23)?,
        altitude: r.get(24)?,
        place_label: r.get(25)?,
        location_source: r.get::<_, Option<String>>(26)?.as_deref().map(parse_location_source),
        place_status: parse_place_status(&r.get::<_, String>(27)?),
        manual_place_override: r.get::<_, i64>(28)? != 0,
        duration_ms: r.get(29)?,
        width: r.get::<_, Option<i64>>(30)?.map(|v| v as u32),
        height: r.get::<_, Option<i64>>(31)?.map(|v| v as u32),
        video_codec: r.get(32)?,
        audio_codec: r.get(33)?,
        scan_timestamp: r.get(34)?,
        last_error: r.get(35)?,
        is_sidecar: r.get::<_, i64>(36)? != 0,
        sidecar_parent_id: r.get(37)?,
    })
}

fn parse_kind(s: &str) -> MediaKind {
    match s {
        "photo" => MediaKind::Photo,
        "video" => MediaKind::Video,
        "sidecar" => MediaKind::Sidecar,
        _ => MediaKind::Other,
    }
}
pub fn kind_str(k: MediaKind) -> &'static str {
    match k {
        MediaKind::Photo => "photo",
        MediaKind::Video => "video",
        MediaKind::Sidecar => "sidecar",
        MediaKind::Other => "other",
    }
}
fn parse_meta_status(s: &str) -> MetadataStatus {
    match s {
        "ok" => MetadataStatus::Ok,
        "unavailable" => MetadataStatus::Unavailable,
        "needs_review" => MetadataStatus::NeedsReview,
        _ => MetadataStatus::Unsupported,
    }
}
pub fn meta_status_str(s: MetadataStatus) -> &'static str {
    match s {
        MetadataStatus::Ok => "ok",
        MetadataStatus::Unavailable => "unavailable",
        MetadataStatus::NeedsReview => "needs_review",
        MetadataStatus::Unsupported => "unsupported",
    }
}
pub fn date_source_str(s: DateSource) -> &'static str {
    match s {
        DateSource::Exif => "exif",
        DateSource::Xmp => "xmp",
        DateSource::Iptc => "iptc",
        DateSource::MediaContainer => "container",
        DateSource::Specialized => "specialized",
        DateSource::FileSystem => "filesystem",
        DateSource::Manual => "manual",
    }
}
fn parse_date_source(s: &str) -> DateSource {
    match s {
        "exif" => DateSource::Exif,
        "xmp" => DateSource::Xmp,
        "iptc" => DateSource::Iptc,
        "container" => DateSource::MediaContainer,
        "specialized" => DateSource::Specialized,
        "filesystem" => DateSource::FileSystem,
        _ => DateSource::Manual,
    }
}
pub fn date_status_str(s: DateStatus) -> &'static str {
    match s {
        DateStatus::Confirmed => "confirmed",
        DateStatus::NeedsReview => "needs_review",
        DateStatus::Fallback => "fallback",
        DateStatus::Unknown => "unknown",
    }
}
fn parse_date_status(s: &str) -> DateStatus {
    match s {
        "confirmed" => DateStatus::Confirmed,
        "needs_review" => DateStatus::NeedsReview,
        "fallback" => DateStatus::Fallback,
        _ => DateStatus::Unknown,
    }
}
pub fn location_source_str(s: LocationSource) -> &'static str {
    match s {
        LocationSource::Manual => "manual",
        LocationSource::Metadata => "metadata",
        LocationSource::Geocoder => "geocoder",
        LocationSource::Coords => "coords",
        LocationSource::None => "none",
    }
}
fn parse_location_source(s: &str) -> LocationSource {
    match s {
        "manual" => LocationSource::Manual,
        "metadata" => LocationSource::Metadata,
        "geocoder" => LocationSource::Geocoder,
        "coords" => LocationSource::Coords,
        _ => LocationSource::None,
    }
}
pub fn place_status_str(s: PlaceStatus) -> &'static str {
    match s {
        PlaceStatus::Resolved => "resolved",
        PlaceStatus::CoordsOnly => "coords_only",
        PlaceStatus::NoGeo => "no_geo",
        PlaceStatus::NeedsReview => "needs_review",
    }
}
fn parse_place_status(s: &str) -> PlaceStatus {
    match s {
        "resolved" => PlaceStatus::Resolved,
        "coords_only" => PlaceStatus::CoordsOnly,
        "needs_review" => PlaceStatus::NeedsReview,
        _ => PlaceStatus::NoGeo,
    }
}

/// Upsert by normalized_path. Manual user overrides (date_source='manual',
/// manual_place_override) survive rescans — spec §7 requires this.
pub fn upsert_media_item(conn: &Connection, item: &MediaItem) -> KResult<()> {
    let existing_manual_date: bool = conn
        .query_row(
            "SELECT date_source FROM media_item WHERE normalized_path = ?1",
            params![item.normalized_path],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten()
        .as_deref()
        == Some("manual");

    conn.execute(
        "INSERT INTO media_item(
            id, source_root_id, path, normalized_path, file_name, extension, media_kind,
            size_bytes, mtime, file_identity, sha256, fingerprint_updated_at, mime_or_detected_type,
            metadata_status, thumbnail_path,
            captured_at_raw, captured_at_local, captured_at_utc, timezone_offset, date_source, date_status, normalization_note,
            latitude, longitude, altitude, place_label, location_source, place_status, manual_place_override,
            duration_ms, width, height, video_codec, audio_codec,
            scan_timestamp, last_error, is_sidecar, sidecar_parent_id)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,
                 ?23,?24,?25,?26,?27,?28,?29,?30,?31,?32,?33,?34,?35,?36,?37,?38)
         ON CONFLICT(normalized_path) DO UPDATE SET
            source_root_id=excluded.source_root_id,
            path=excluded.path, file_name=excluded.file_name, extension=excluded.extension,
            media_kind=excluded.media_kind,
            size_bytes=excluded.size_bytes, mtime=excluded.mtime,
            file_identity=excluded.file_identity,
            sha256=CASE WHEN excluded.sha256 IS NULL THEN media_item.sha256 ELSE excluded.sha256 END,
            fingerprint_updated_at=excluded.fingerprint_updated_at,
            mime_or_detected_type=excluded.mime_or_detected_type,
            metadata_status=excluded.metadata_status,
            thumbnail_path=excluded.thumbnail_path,
            captured_at_raw=CASE WHEN media_item.date_source='manual' THEN media_item.captured_at_raw ELSE excluded.captured_at_raw END,
            captured_at_local=CASE WHEN media_item.date_source='manual' THEN media_item.captured_at_local ELSE excluded.captured_at_local END,
            captured_at_utc=CASE WHEN media_item.date_source='manual' THEN media_item.captured_at_utc ELSE excluded.captured_at_utc END,
            timezone_offset=CASE WHEN media_item.date_source='manual' THEN media_item.timezone_offset ELSE excluded.timezone_offset END,
            date_source=CASE WHEN media_item.date_source='manual' THEN media_item.date_source ELSE excluded.date_source END,
            date_status=CASE WHEN media_item.date_source='manual' THEN media_item.date_status ELSE excluded.date_status END,
            normalization_note=CASE WHEN media_item.date_source='manual' THEN media_item.normalization_note ELSE excluded.normalization_note END,
            latitude=CASE WHEN media_item.manual_place_override=1 THEN media_item.latitude ELSE excluded.latitude END,
            longitude=CASE WHEN media_item.manual_place_override=1 THEN media_item.longitude ELSE excluded.longitude END,
            altitude=CASE WHEN media_item.manual_place_override=1 THEN media_item.altitude ELSE excluded.altitude END,
            place_label=CASE WHEN media_item.manual_place_override=1 THEN media_item.place_label ELSE excluded.place_label END,
            location_source=CASE WHEN media_item.manual_place_override=1 THEN media_item.location_source ELSE excluded.location_source END,
            place_status=CASE WHEN media_item.manual_place_override=1 THEN media_item.place_status ELSE excluded.place_status END,
            manual_place_override=media_item.manual_place_override,
            duration_ms=excluded.duration_ms, width=excluded.width, height=excluded.height,
            video_codec=excluded.video_codec, audio_codec=excluded.audio_codec,
            scan_timestamp=excluded.scan_timestamp,
            last_error=excluded.last_error,
            is_sidecar=excluded.is_sidecar,
            sidecar_parent_id=excluded.sidecar_parent_id",
        params![
            item.id, item.source_root_id, item.path, item.normalized_path, item.file_name,
            item.extension, kind_str(item.media_kind), item.size_bytes, item.mtime,
            item.file_identity, item.sha256, item.fingerprint_updated_at, item.mime_or_detected_type,
            meta_status_str(item.metadata_status), item.thumbnail_path,
            item.captured_at_raw, item.captured_at_local, item.captured_at_utc, item.timezone_offset,
            item.date_source.map(date_source_str), date_status_str(item.date_status), item.normalization_note,
            item.latitude, item.longitude, item.altitude, item.place_label,
            item.location_source.map(location_source_str), place_status_str(item.place_status),
            item.manual_place_override as i64,
            item.duration_ms, item.width.map(|v| v as i64), item.height.map(|v| v as i64),
            item.video_codec, item.audio_codec, item.scan_timestamp, item.last_error,
            item.is_sidecar as i64, item.sidecar_parent_id,
        ],
    )?;
    let _ = existing_manual_date; // preserved by SQL CASE logic above
    Ok(())
}

pub fn get_media_item(conn: &Connection, id: &str) -> KResult<Option<MediaItem>> {
    let sql = format!("SELECT {MEDIA_COLS} FROM media_item WHERE id = ?1");
    let mut stmt = conn.prepare(&sql)?;
    Ok(stmt.query_row(params![id], row_to_media).optional()?)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MediaFilter {
    pub search: Option<String>,
    pub kind: Option<String>, // photo|video|sidecar
    pub date_status: Option<String>,
    pub place_status: Option<String>,
    pub needs_review: Option<bool>,
    pub duplicates_only: Option<bool>,
    pub root_id: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub sort: Option<String>, // date|name|size (default date desc)
}

pub fn query_media(conn: &Connection, f: &MediaFilter) -> KResult<(Vec<MediaItem>, i64)> {
    let mut where_sql: Vec<String> = Vec::new();
    let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(s) = &f.search {
        where_sql.push("(file_name LIKE ? OR place_label LIKE ? OR path LIKE ?)".into());
        let pat = format!("%{s}%");
        args.push(Box::new(pat.clone()));
        args.push(Box::new(pat.clone()));
        args.push(Box::new(pat));
    }
    if let Some(k) = &f.kind {
        where_sql.push("media_kind = ?".into());
        args.push(Box::new(k.clone()));
    }
    if let Some(d) = &f.date_status {
        where_sql.push("date_status = ?".into());
        args.push(Box::new(d.clone()));
    }
    if let Some(p) = &f.place_status {
        where_sql.push("place_status = ?".into());
        args.push(Box::new(p.clone()));
    }
    if f.needs_review == Some(true) {
        where_sql.push("(date_status='needs_review' OR place_status='needs_review' OR metadata_status='needs_review')".into());
    }
    if f.duplicates_only == Some(true) {
        where_sql.push(
            "sha256 IS NOT NULL AND sha256 IN (SELECT sha256 FROM media_item WHERE sha256 IS NOT NULL GROUP BY sha256 HAVING COUNT(*) > 1)"
                .into(),
        );
    }
    if let Some(r) = &f.root_id {
        where_sql.push("source_root_id = ?".into());
        args.push(Box::new(r.clone()));
    }
    // gallery hides sidecars by default (they travel with the parent)
    if f.kind.is_none() {
        where_sql.push("media_kind != 'sidecar'".into());
    }

    let w = if where_sql.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", where_sql.join(" AND "))
    };
    let total: i64 =
        conn.query_row(&format!("SELECT COUNT(*) FROM media_item{w}"), rusqlite::params_from_iter(args.iter()), |r| r.get(0))?;

    let order = match f.sort.as_deref() {
        Some("name") => "file_name COLLATE NOCASE ASC",
        Some("size") => "size_bytes DESC",
        Some("name_desc") => "file_name COLLATE NOCASE DESC",
        Some("size_asc") => "size_bytes ASC",
        _ => "COALESCE(captured_at_local, '9999') DESC, file_name ASC",
    };
    let limit = f.limit.unwrap_or(200).min(2000);
    let offset = f.offset.unwrap_or(0);
    let sql = format!("SELECT {MEDIA_COLS} FROM media_item{w} ORDER BY {order} LIMIT ? OFFSET ?");
    args.push(Box::new(limit));
    args.push(Box::new(offset));
    let mut stmt = conn.prepare(&sql)?;
    let items = stmt
        .query_map(rusqlite::params_from_iter(args.iter()), row_to_media)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok((items, total))
}

pub fn set_manual_place(conn: &Connection, ids: &[String], place: &str) -> KResult<()> {
    let place = place.trim();
    let status = if place.is_empty() { "no_geo" } else { "resolved" };
    for id in ids {
        conn.execute(
            "UPDATE media_item SET place_label = ?1, location_source = 'manual',
             place_status = ?2, manual_place_override = 1 WHERE id = ?3",
            params![if place.is_empty() { None } else { Some(place) }, status, id],
        )?;
    }
    Ok(())
}

pub fn set_manual_date(conn: &Connection, id: &str, raw: &str) -> KResult<()> {
    use crate::domain::captured_date::from_manual;
    let d = from_manual(raw);
    conn.execute(
        "UPDATE media_item SET captured_at_raw=?1, captured_at_local=?2, captured_at_utc=?3,
         timezone_offset=?4, date_source='manual', date_status=?5, normalization_note=?6 WHERE id=?7",
        params![
            d.captured_at_raw,
            d.captured_at_local.map(|v| v.format("%Y-%m-%dT%H:%M:%S").to_string()),
            d.captured_at_utc.map(|v| v.to_rfc3339()),
            d.tz_offset_min,
            date_status_str(d.date_status),
            d.normalization_note,
            id
        ],
    )?;
    Ok(())
}

pub fn delete_missing_under(conn: &Connection, root_id: &str, scan_started_ms: i64) -> KResult<usize> {
    Ok(conn.execute(
        "DELETE FROM media_item WHERE source_root_id = ?1 AND scan_timestamp < ?2",
        params![root_id, scan_started_ms],
    )?)
}

// ---------- scan jobs ----------

pub fn create_scan_job(conn: &Connection, id: &str, root_id: &str) -> KResult<()> {
    conn.execute(
        "INSERT INTO scan_job(id, root_id, state, started_at) VALUES (?1, ?2, 'running', ?3)",
        params![id, root_id, now_ms()],
    )?;
    Ok(())
}

pub fn update_scan_progress(
    conn: &Connection,
    id: &str,
    found: i64,
    processed: i64,
    skipped: i64,
    errors: i64,
    current: &str,
) -> KResult<()> {
    conn.execute(
        "UPDATE scan_job SET found=?1, processed=?2, skipped=?3, errors=?4, current_file=?5 WHERE id=?6",
        params![found, processed, skipped, errors, current, id],
    )?;
    Ok(())
}

pub fn finish_scan_job(conn: &Connection, id: &str, cancelled: bool) -> KResult<()> {
    conn.execute(
        "UPDATE scan_job SET state=?1, cancelled=?2, finished_at=?3 WHERE id=?4",
        params![if cancelled { "cancelled" } else { "done" }, cancelled as i64, now_ms(), id],
    )?;
    Ok(())
}

// ---------- plans / operations ----------

pub fn save_plan(conn: &Connection, plan_id: &str, mode: OperationMode, dest_root: &str, template: &str, settings_json: &str) -> KResult<()> {
    conn.execute(
        "INSERT INTO operation_plan(id, created_at, mode, destination_root, template, settings_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![plan_id, now_ms(), if mode == OperationMode::Copy { "copy" } else { "move" }, dest_root, template, settings_json],
    )?;
    Ok(())
}

pub fn save_plan_items(conn: &Connection, items: &[PlanItem]) -> KResult<()> {
    for it in items {
        conn.execute(
            "INSERT INTO operation_item(id, plan_id, media_id, source_path, destination_path, mode, status,
                size_bytes, sha256_before, warnings_json, sidecar_of)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
             ON CONFLICT(id) DO UPDATE SET destination_path=excluded.destination_path,
                status=excluded.status, warnings_json=excluded.warnings_json",
            params![
                it.id, it.plan_id, it.media_id, it.source_path, it.destination_path,
                if it.mode == OperationMode::Copy { "copy" } else { "move" },
                plan_status_str(it.status), it.size_bytes, it.sha256_before,
                serde_json::to_string(&it.warnings).unwrap_or_default(), it.sidecar_of,
            ],
        )?;
    }
    Ok(())
}

pub fn plan_status_str(s: PlanItemStatus) -> &'static str {
    match s {
        PlanItemStatus::Pending => "pending",
        PlanItemStatus::Excluded => "excluded",
        PlanItemStatus::Done => "done",
        PlanItemStatus::Skipped => "skipped",
        PlanItemStatus::Failed => "failed",
        PlanItemStatus::Undone => "undone",
        PlanItemStatus::NeedsUser => "needs_user",
    }
}
fn parse_plan_status(s: &str) -> PlanItemStatus {
    match s {
        "pending" => PlanItemStatus::Pending,
        "excluded" => PlanItemStatus::Excluded,
        "done" => PlanItemStatus::Done,
        "skipped" => PlanItemStatus::Skipped,
        "failed" => PlanItemStatus::Failed,
        "undone" => PlanItemStatus::Undone,
        _ => PlanItemStatus::NeedsUser,
    }
}

pub fn get_plan_items(conn: &Connection, plan_id: &str) -> KResult<Vec<PlanItem>> {
    let mut stmt = conn.prepare(
        "SELECT id, plan_id, media_id, source_path, destination_path, mode, status, size_bytes,
                sha256_before, sha256_after, warnings_json, sidecar_of, error, finished_at,
                undo_source_path, undo_dest_path
         FROM operation_item WHERE plan_id = ?1",
    )?;
    let items = stmt
        .query_map(params![plan_id], |r| {
            let mode = if r.get::<_, String>(5)? == "copy" { OperationMode::Copy } else { OperationMode::Move };
            let warnings: Vec<PlanWarning> =
                serde_json::from_str(&r.get::<_, Option<String>>(10)?.unwrap_or_default()).unwrap_or_default();
            Ok(PlanItem {
                id: r.get(0)?,
                plan_id: r.get(1)?,
                media_id: r.get(2)?,
                source_path: r.get(3)?,
                destination_path: r.get(4)?,
                mode,
                status: parse_plan_status(&r.get::<_, String>(6)?),
                size_bytes: r.get(7)?,
                sha256_before: r.get(8)?,
                sha256_after: r.get(9)?,
                warnings,
                sidecar_of: r.get(11)?,
                place_override: None,
                error: r.get(12)?,
                finished_at: r.get(13)?,
                undo_source_path: r.get(14)?,
                undo_dest_path: r.get(15)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(items)
}

pub fn update_plan_item_destination(conn: &Connection, item_id: &str, dest: &str) -> KResult<()> {
    conn.execute("UPDATE operation_item SET destination_path=?1 WHERE id=?2", params![dest, item_id])?;
    Ok(())
}

pub fn update_plan_item_status(conn: &Connection, item_id: &str, status: PlanItemStatus) -> KResult<()> {
    conn.execute("UPDATE operation_item SET status=?1 WHERE id=?2", params![plan_status_str(status), item_id])?;
    Ok(())
}

pub fn update_plan_item_mode(conn: &Connection, plan_id: &str, mode: OperationMode) -> KResult<()> {
    let m = if mode == OperationMode::Copy { "copy" } else { "move" };
    conn.execute("UPDATE operation_item SET mode=?1 WHERE plan_id=?2", params![m, plan_id])?;
    conn.execute("UPDATE operation_plan SET mode=?1 WHERE id=?2", params![m, plan_id])?;
    Ok(())
}

pub fn set_plan_state(conn: &Connection, plan_id: &str, state: &str) -> KResult<()> {
    conn.execute("UPDATE operation_plan SET state=?1 WHERE id=?2", params![state, plan_id])?;
    Ok(())
}

pub fn get_plan_meta(conn: &Connection, plan_id: &str) -> KResult<Option<(String, String, String)>> {
    Ok(conn
        .query_row(
            "SELECT mode, destination_root, state FROM operation_plan WHERE id=?1",
            params![plan_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?)
}

// ---------- log ----------

pub fn log_event(conn: &Connection, job_id: &str, level: LogLevel, action: &str, message: &str) -> KResult<()> {
    let lvl = match level {
        LogLevel::Info => "info",
        LogLevel::Warn => "warn",
        LogLevel::Error => "error",
    };
    conn.execute(
        "INSERT INTO operation_log(job_id, level, action, message, created_at) VALUES (?1,?2,?3,?4,?5)",
        params![job_id, lvl, action, message, now_ms()],
    )?;
    Ok(())
}

pub fn recent_logs(conn: &Connection, limit: i64) -> KResult<Vec<OperationLogEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, job_id, level, action, message, created_at FROM operation_log ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = stmt
        .query_map(params![limit], |r| {
            Ok(OperationLogEntry {
                id: r.get(0)?,
                job_id: r.get(1)?,
                level: match r.get::<_, String>(2)?.as_str() {
                    "warn" => LogLevel::Warn,
                    "error" => LogLevel::Error,
                    _ => LogLevel::Info,
                },
                action: r.get(3)?,
                message: r.get(4)?,
                created_at: r.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

// ---------- settings ----------

pub fn get_setting(conn: &Connection, key: &str) -> KResult<Option<String>> {
    Ok(conn.query_row("SELECT value FROM app_settings WHERE key=?1", params![key], |r| r.get(0)).optional()?)
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> KResult<()> {
    conn.execute(
        "INSERT INTO app_settings(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=?2",
        params![key, value],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(path: &str) -> MediaItem {
        MediaItem {
            id: uuid::Uuid::new_v4().to_string(),
            source_root_id: "root1".into(),
            path: path.into(),
            normalized_path: path.to_lowercase(),
            file_name: "a.jpg".into(),
            extension: "jpg".into(),
            media_kind: MediaKind::Photo,
            size_bytes: 10,
            mtime: 1,
            file_identity: None,
            sha256: None,
            fingerprint_updated_at: None,
            mime_or_detected_type: None,
            metadata_status: MetadataStatus::Ok,
            thumbnail_path: None,
            captured_at_raw: Some("2025:08:12 14:03:22".into()),
            captured_at_local: Some("2025-08-12T14:03:22".into()),
            captured_at_utc: None,
            timezone_offset: None,
            date_source: Some(DateSource::Exif),
            date_status: DateStatus::Confirmed,
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
            scan_timestamp: 100,
            last_error: None,
            is_sidecar: false,
            sidecar_parent_id: None,
        }
    }

    #[test]
    fn upsert_no_duplicates_and_preserves_overrides() {
        let conn = open_db(":memory:").unwrap();
        let root = add_source_root(&conn, "/data").unwrap();
        let mut item = sample("/data/A.jpg");
        item.source_root_id = root.id.clone();
        upsert_media_item(&conn, &item).unwrap();
        // rescan same path with different id
        item.id = uuid::Uuid::new_v4().to_string();
        item.scan_timestamp = 200;
        upsert_media_item(&conn, &item).unwrap();
        let (rows, total) = query_media(&conn, &MediaFilter::default()).unwrap();
        assert_eq!(total, 1);
        assert_eq!(rows[0].scan_timestamp, 200);

        // manual place override
        let id = rows[0].id.clone();
        set_manual_place(&conn, &[id.clone()], "Карпати").unwrap();
        // rescan again
        item.id = uuid::Uuid::new_v4().to_string();
        upsert_media_item(&conn, &item).unwrap();
        let got = get_media_item(&conn, &id).unwrap().unwrap();
        assert_eq!(got.place_label.as_deref(), Some("Карпати"));
        assert!(got.manual_place_override);
        assert_eq!(got.location_source, Some(LocationSource::Manual));

        // manual date override survives rescan
        set_manual_date(&conn, &id, "2020-05-01 10:00:00").unwrap();
        item.id = uuid::Uuid::new_v4().to_string();
        upsert_media_item(&conn, &item).unwrap();
        let got = get_media_item(&conn, &id).unwrap().unwrap();
        assert_eq!(got.date_source, Some(DateSource::Manual));
        assert_eq!(got.captured_at_local.as_deref(), Some("2020-05-01T10:00:00"));
    }

    #[test]
    fn duplicate_query_groups() {
        let conn = open_db(":memory:").unwrap();
        let root = add_source_root(&conn, "/x").unwrap();
        let mut a = sample("/x/1.jpg");
        a.source_root_id = root.id.clone();
        a.sha256 = Some("hash-a".into());
        let mut b = sample("/x/2.jpg");
        b.source_root_id = root.id.clone();
        b.id = uuid::Uuid::new_v4().to_string();
        b.sha256 = Some("hash-a".into());
        let mut c = sample("/x/3.jpg");
        c.source_root_id = root.id.clone();
        c.id = uuid::Uuid::new_v4().to_string();
        c.sha256 = Some("hash-b".into());
        for it in [&a, &b, &c] {
            upsert_media_item(&conn, it).unwrap();
        }
        let (_, total) = query_media(&conn, &MediaFilter { duplicates_only: Some(true), ..Default::default() }).unwrap();
        assert_eq!(total, 2);
    }

    #[test]
    fn settings_roundtrip() {
        let conn = open_db(":memory:").unwrap();
        set_setting(&conn, "theme", "dark").unwrap();
        assert_eq!(get_setting(&conn, "theme").unwrap().as_deref(), Some("dark"));
        set_setting(&conn, "theme", "light").unwrap();
        assert_eq!(get_setting(&conn, "theme").unwrap().as_deref(), Some("light"));
    }
}
