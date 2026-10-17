//! Exact duplicate detection via SHA-256 (size is only a pre-filter — §10).

use crate::domain::media_item::MediaItem;
use crate::persistence::db::{self, MEDIA_COLS};
use crate::errors::KResult;
use crate::services::scanner::sha256_file;
use rusqlite::Connection;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// Groups with 2+ files sharing identical SHA-256.
pub fn duplicate_groups(conn: &Connection) -> KResult<Vec<Vec<MediaItem>>> {
    let hashes: Vec<String> = conn
        .prepare(
            "SELECT sha256 FROM media_item WHERE sha256 IS NOT NULL GROUP BY sha256 HAVING COUNT(*) > 1 ORDER BY COUNT(*) DESC",
        )?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let mut groups = Vec::new();
    let mut stmt = conn.prepare(&format!(
        "SELECT {MEDIA_COLS} FROM media_item WHERE sha256 = ?1 ORDER BY size_bytes DESC, path"
    ))?;
    for h in hashes {
        let mut items = Vec::new();
        let mut mapped = stmt.query_map(rusqlite::params![h], db::row_to_media)?;
        while let Some(r) = mapped.next() {
            items.push(r?);
        }
        groups.push(items);
    }
    Ok(groups)
}

/// Compute SHA-256 for items missing it (hash-on-demand path used by the
/// Duplicates view when scanning ran with hashing disabled).
pub fn ensure_hashes(conn: &Connection, cancel: &AtomicBool, limit: i64, mut on_progress: impl FnMut(usize, usize)) -> KResult<usize> {
    let rows: Vec<(String, String, i64, i64)> = conn
        .prepare("SELECT id, path, size_bytes, mtime FROM media_item WHERE sha256 IS NULL LIMIT ?1")?
        .query_map(rusqlite::params![limit], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<Result<_, _>>()?;
    let mut done = 0;
    for (id, path, _size, _mtime) in rows {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        match sha256_file(Path::new(&path)) {
            Ok(h) => {
                conn.execute(
                    "UPDATE media_item SET sha256=?1, fingerprint_updated_at=?2 WHERE id=?3",
                    rusqlite::params![h, db::now_ms(), id],
                )?;
            }
            Err(e) => {
                conn.execute("UPDATE media_item SET last_error=?1 WHERE id=?2", rusqlite::params![e.friendly(), id])?;
            }
        }
        done += 1;
        on_progress(done, limit as usize);
    }
    Ok(done)
}

pub fn hash_group_count(conn: &Connection) -> KResult<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM (SELECT sha256 FROM media_item WHERE sha256 IS NOT NULL GROUP BY sha256 HAVING COUNT(*) > 1)",
        [],
        |r| r.get(0),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::db::{add_source_root, open_db};
    use crate::services::scanner::{scan_root, ScanOptions};
    use std::sync::atomic::AtomicBool;

    #[test]
    fn exact_dups_grouped() {
        let dir = std::env::temp_dir().join(format!("kadr-dup-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::create_dir_all(dir.join("b")).unwrap();
        std::fs::write(dir.join("a/x.jpg"), b"same-bytes").unwrap();
        std::fs::write(dir.join("b/y.jpg"), b"same-bytes").unwrap();
        std::fs::write(dir.join("a/z.jpg"), b"different").unwrap();
        let conn = open_db(":memory:").unwrap();
        let root = add_source_root(&conn, dir.to_str().unwrap()).unwrap();
        let cancel = AtomicBool::new(false);
        scan_root(&conn, &root, &ScanOptions::default(), "j", &cancel, &|_| {}).unwrap();
        let groups = duplicate_groups(&conn).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].len(), 2);
        assert_eq!(hash_group_count(&conn).unwrap(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn hash_on_demand_when_scan_skipped_hashes() {
        let dir = std::env::temp_dir().join(format!("kadr-dup2-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("q.jpg"), b"abc").unwrap();
        let conn = open_db(":memory:").unwrap();
        let root = add_source_root(&conn, dir.to_str().unwrap()).unwrap();
        let cancel = AtomicBool::new(false);
        scan_root(&conn, &root, &ScanOptions { compute_hashes: false, ..Default::default() }, "j", &cancel, &|_| {}).unwrap();
        let n = ensure_hashes(&conn, &cancel, 10, |_, _| {}).unwrap();
        assert_eq!(n, 1);
        let h: Option<String> = conn.query_row("SELECT sha256 FROM media_item", [], |r| r.get(0)).unwrap();
        assert!(h.is_some());
        std::fs::remove_dir_all(&dir).ok();
    }
}
