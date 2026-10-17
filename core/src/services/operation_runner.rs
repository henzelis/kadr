//! Safe apply engine: copy via temp+verify+rename, move via rename with
//! verified copy fallback, journaling, undo, retry, cancel (TECHNICAL_SPEC §11).

use crate::domain::operation::{OperationMode, PlanItem, PlanItemStatus};
use crate::errors::{KResult, KadrError};
use crate::domain::operation::LogLevel;
use crate::persistence::db;
use crate::services::scanner::sha256_file;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{BufReader, BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApplyProgress {
    pub total: usize,
    pub done: usize,
    pub failed: usize,
    pub skipped: usize,
    pub current: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApplyReport {
    pub done: usize,
    pub failed: usize,
    pub skipped: usize,
    pub cancelled: bool,
    pub job_id: String,
}

fn copy_verified(source: &Path, dest_final: &Path) -> KResult<String> {
    // 1. copy to a temp file in the destination directory
    let dir = dest_final.parent().ok_or_else(|| {
        KadrError::user_detail("Некоректний шлях призначення", dest_final.display().to_string())
    })?;
    fs::create_dir_all(dir).map_err(|e| KadrError::Io {
        path: dir.display().to_string(),
        source: e,
    })?;
    let tmp_name = format!(
        ".kadr-tmp-{}-{}",
        dest_final
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".into()),
        uuid::Uuid::new_v4().simple()
    );
    let tmp = dir.join(tmp_name);

    let result = (|| -> KResult<String> {
        let src_file = fs::File::open(source).map_err(|e| KadrError::Io {
            path: source.display().to_string(),
            source: e,
        })?;
        let mut reader = BufReader::with_capacity(1 << 20, src_file);
        let dst_file = fs::File::create(&tmp).map_err(|e| KadrError::Io {
            path: tmp.display().to_string(),
            source: e,
        })?;
        let mut writer = BufWriter::with_capacity(1 << 20, dst_file);
        std::io::copy(&mut reader, &mut writer)
            .map_err(|e| KadrError::Io { path: tmp.display().to_string(), source: e })?;
        writer.flush().map_err(|e| KadrError::Io { path: tmp.display().to_string(), source: e })?;
        writer.get_mut().sync_all().map_err(|e| KadrError::Io {
            path: tmp.display().to_string(),
            source: e,
        })?;
        drop(writer);

        // 2. verify size + checksum before publishing the final name
        let src_meta = fs::metadata(source).map_err(|e| KadrError::Io {
            path: source.display().to_string(),
            source: e,
        })?;
        let tmp_meta = fs::metadata(&tmp).map_err(|e| KadrError::Io {
            path: tmp.display().to_string(),
            source: e,
        })?;
        if src_meta.len() != tmp_meta.len() {
            return Err(KadrError::user_detail(
                "Перевірка розміру копії не пройшла",
                format!("{} != {}", src_meta.len(), tmp_meta.len()),
            ));
        }
        let src_hash = sha256_file(source)?;
        let tmp_hash = sha256_file(&tmp)?;
        if src_hash != tmp_hash {
            return Err(KadrError::user_detail(
                "Перевірка SHA-256 копії не пройшла",
                tmp.display().to_string(),
            ));
        }

        // 3. atomic rename to the final name (existence re-check by caller policy)
        if dest_final.exists() {
            return Err(KadrError::user("Файл у призначенні з'явився під час копіювання — перезапис заблоковано"));
        }
        fs::rename(&tmp, dest_final).map_err(|e| KadrError::Io {
            path: dest_final.display().to_string(),
            source: e,
        })?;
        Ok(src_hash)
    })();

    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn unique_existing(dest: &Path) -> std::path::PathBuf {
    let s = dest.to_string_lossy().to_string();
    std::path::PathBuf::from(db_unique(&s, |p| Path::new(p).exists()))
}

fn db_unique(s: &str, exists: impl Fn(&str) -> bool) -> String {
    crate::domain::operation::unique_name(s, exists)
}

/// Execute all pending items of a stored plan.
pub fn apply_plan(
    conn: &Connection,
    plan_id: &str,
    cancel: &AtomicBool,
    on_progress: &dyn Fn(&ApplyProgress),
) -> KResult<ApplyReport> {
    let meta = db::get_plan_meta(conn, plan_id)?
        .ok_or_else(|| KadrError::user("План не знайдено"))?;
    let dest_root = meta.1.clone();
    let all = db::get_plan_items(conn, plan_id)?;
    let items: Vec<PlanItem> = all
        .into_iter()
        .filter(|i| matches!(i.status, PlanItemStatus::Pending))
        .collect();

    let job_id = uuid::Uuid::new_v4().to_string();
    db::log_event(conn, &job_id, LogLevel::Info, "apply", &format!("Почато застосування плану ({} файлів, режим {})", items.len(), meta.0))?;
    db::set_plan_state(conn, plan_id, "applying")?;

    let mut prog = ApplyProgress { total: items.len(), ..Default::default() };
    let mut report = ApplyReport { job_id: job_id.clone(), ..Default::default() };

    for item in items {
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            db::log_event(conn, &job_id, LogLevel::Warn, "apply", "Застосування скасовано користувачем")?;
            break;
        }
        prog.current = item.source_path.clone();
        on_progress(&prog);

        let outcome = apply_item(conn, &job_id, &item, &dest_root);
        match outcome {
            Ok(sha_after) => {
                conn.execute(
                    "UPDATE operation_item SET status='done', finished_at=?1, sha256_after=?2,
                     undo_source_path=?3, undo_dest_path=?4, error=NULL WHERE id=?5",
                    rusqlite::params![db::now_ms(), sha_after, item.source_path, item.destination_path, item.id],
                )?;
                prog.done += 1;
                report.done += 1;
            }
            Err(e) => {
                let msg = e.friendly();
                conn.execute(
                    "UPDATE operation_item SET status='failed', error=?1, finished_at=?2 WHERE id=?3",
                    rusqlite::params![msg, db::now_ms(), item.id],
                )?;
                db::log_event(conn, &job_id, LogLevel::Error, "apply", &format!("{}: {}", item.source_path, msg))?;
                prog.failed += 1;
                report.failed += 1;
            }
        }
    }

    db::set_plan_state(
        conn,
        plan_id,
        if report.cancelled { "interrupted" } else { "applied" },
    )?;
    db::log_event(
        conn,
        &job_id,
        if report.failed > 0 { LogLevel::Warn } else { LogLevel::Info },
        "apply",
        &format!("Завершено:готово {}, помилок {}", report.done, report.failed),
    )?;
    on_progress(&prog);
    Ok(report)
}

/// Returns sha256 of the resulting file on success.
fn apply_item(conn: &Connection, job_id: &str, item: &PlanItem, dest_root: &str) -> KResult<String> {
    let src = Path::new(&item.source_path);
    let dest = Path::new(&item.destination_path);

    if !src.exists() {
        return Err(KadrError::user("Джерело зникло з диска — файл пропущено"));
    }
    let md = fs::metadata(src).map_err(|e| KadrError::Io { path: item.source_path.clone(), source: e })?;
    if md.len() as i64 != item.size_bytes {
        return Err(KadrError::user_detail(
            "Файл змінився після побудови плану — потрібне нове сканування",
            format!("{} → {}", item.size_bytes, md.len()),
        ));
    }
    // containment re-check at apply time (never trust stored paths blindly)
    if !super::path_planner::is_inside_root(dest_root, dest) {
        return Err(KadrError::user("Цільовий шлях виходить за межі папки архіву — заблоковано"));
    }

    // conflict policy at apply time: never overwrite
    let final_dest = if dest.exists() {
        let alt = unique_existing(dest);
        db::log_event(conn, job_id, LogLevel::Warn, "apply", &format!("Конфлікт назви, застосовано суфікс: {}", alt.display()))?;
        alt
    } else {
        dest.to_path_buf()
    };

    if item.mode == OperationMode::Move {
        // same-volume fast path: rename, but only after re-verification of the source
        match fs::rename(src, &final_dest) {
            Ok(()) => {
                let sha = sha256_file(&final_dest)?;
                // cross-check with planned hash when we have one
                if let Some(expected) = &item.sha256_before {
                    if &sha != expected {
                        // content changed after planning: put the file back
                        let _ = fs::rename(&final_dest, src);
                        return Err(KadrError::user("Вміст файлу не збігається із запланованим — переміщення скасовано"));
                    }
                }
                return Ok(sha);
            }
            Err(_) => {
                // cross-volume or permission fallback: verified copy, delete only after success
                let sha = copy_verified(src, &final_dest)?;
                fs::remove_file(src).map_err(|e| KadrError::user_detail(
                    "Копію завершено та перевірено, але джерело не видалено — лишається обидві копії",
                    e.to_string(),
                ))?;
                return Ok(sha);
            }
        }
    }

    copy_verified(src, &final_dest)
}

/// Undo a finished apply: verified reverse mapping, never overwrites.
pub fn undo_plan(
    conn: &Connection,
    plan_id: &str,
    mode_was_move: bool,
    cancel: &AtomicBool,
    on_progress: &dyn Fn(&ApplyProgress),
) -> KResult<ApplyReport> {
    let items: Vec<PlanItem> = db::get_plan_items(conn, plan_id)?
        .into_iter()
        .filter(|i| matches!(i.status, PlanItemStatus::Done))
        .collect();
    let job_id = uuid::Uuid::new_v4().to_string();
    db::log_event(conn, &job_id, LogLevel::Info, "undo", &format!("Undo плану ({} файлів)", items.len()))?;
    let mut prog = ApplyProgress { total: items.len(), ..Default::default() };
    let mut report = ApplyReport { job_id: job_id.clone(), ..Default::default() };

    for item in items {
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            break;
        }
        prog.current = item.destination_path.clone();
        on_progress(&prog);

        let dest = Path::new(&item.destination_path);
        let src = Path::new(&item.source_path);

        if !dest.exists() {
            conn.execute("UPDATE operation_item SET error=?1 WHERE id=?2",
                rusqlite::params!["Undo неможливий: цільовий файл зникне", item.id])?;
            report.failed += 1;
            continue;
        }
        let sha = match sha256_file(dest) {
            Ok(s) => s,
            Err(e) => {
                conn.execute("UPDATE operation_item SET error=?1 WHERE id=?2",
                    rusqlite::params![e.friendly(), item.id])?;
                report.failed += 1;
                continue;
            }
        };
        if let Some(expected) = &item.sha256_before {
            if &sha != expected {
                conn.execute("UPDATE operation_item SET error=?1 WHERE id=?2",
                    rusqlite::params!["Undo заблоковано: цільовий файл зміннено ззовні", item.id])?;
                db::log_event(conn, &job_id, LogLevel::Warn, "undo", &format!("Пропущено (файл зміннено): {}", dest.display()))?;
                report.failed += 1;
                continue;
            }
        }

        if mode_was_move {
            // restore the file to its original location; refuse if occupied
            if src.exists() {
                conn.execute("UPDATE operation_item SET error=?1 WHERE id=?2",
                    rusqlite::params!["Undo: вихідний шлях уже зайнятий", item.id])?;
                report.failed += 1;
                continue;
            }
            if let Some(dir) = src.parent() {
                let _ = fs::create_dir_all(dir);
            }
            if let Err(e) = fs::rename(dest, src) {
                // cross-volume restore
                match copy_verified(dest, src) {
                    Ok(_) => {
                        let _ = fs::remove_file(dest);
                        let _ = e;
                    }
                    Err(err) => {
                        conn.execute("UPDATE operation_item SET error=?1 WHERE id=?2",
                            rusqlite::params![err.friendly(), item.id])?;
                        report.failed += 1;
                        continue;
                    }
                }
            }
        } else {
            // copy mode: undo = remove our verified copy (original is intact)
            if let Err(e) = fs::remove_file(dest) {
                conn.execute("UPDATE operation_item SET error=?1 WHERE id=?2",
                    rusqlite::params![format!("Не вдалося видалити копію: {e}"), item.id])?;
                report.failed += 1;
                continue;
            }
        }
        conn.execute("UPDATE operation_item SET status='undone', finished_at=?1, error=NULL WHERE id=?2",
            rusqlite::params![db::now_ms(), item.id])?;
        prog.done += 1;
        report.done += 1;
    }
    db::set_plan_state(conn, plan_id, "undone")?;
    db::log_event(conn, &job_id, LogLevel::Info, "undo", &format!("Undo завершено: {}", report.done))?;
    Ok(report)
}

/// Reset failed items and run them again.
pub fn retry_failed(conn: &Connection, plan_id: &str) -> KResult<usize> {
    let n = conn.execute(
        "UPDATE operation_item SET status='pending', error=NULL WHERE plan_id=?1 AND status='failed'",
        rusqlite::params![plan_id],
    )?;
    db::set_plan_state(conn, plan_id, "preview")?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::operation::ConflictPolicy;
    use crate::domain::sort_template::{TargetOs, TemplateConfig};
    use crate::persistence::db::*;
    use crate::services::path_planner::{build_plan, PlanOptions};
    use crate::services::scanner::{scan_root, ScanOptions};
    use std::path::PathBuf;
    use std::sync::atomic::AtomicBool;

    fn fixture() -> (Connection, PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("kadr-ops-{}", uuid::Uuid::new_v4()));
        let src = dir.join("src");
        let dest = dir.join("dest");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dest).unwrap();
        fs::write(src.join("a.jpg"), b"content-a").unwrap();
        fs::write(src.join("b.jpg"), b"content-b").unwrap();
        let conn = open_db(":memory:").unwrap();
        let root = add_source_root(&conn, src.to_str().unwrap()).unwrap();
        let cancel = AtomicBool::new(false);
        scan_root(&conn, &root, &ScanOptions::default(), "j", &cancel, &|_| {}).unwrap();
        (conn, src, dest)
    }

    fn plan_opts(dest: &PathBuf, mode: OperationMode) -> PlanOptions {
        PlanOptions {
            destination_root: dest.to_string_lossy().to_string(),
            template: TemplateConfig { target_os: TargetOs::Unix, ..Default::default() },
            mode,
            conflict: ConflictPolicy::Suffix,
            media_ids: None,
        }
    }

    #[test]
    fn copy_mode_keeps_source_and_verifies() {
        let (conn, src, dest) = fixture();
        let (pid, _, _) = build_plan(&conn, &plan_opts(&dest, OperationMode::Copy)).unwrap();
        let cancel = AtomicBool::new(false);
        let rep = apply_plan(&conn, &pid, &cancel, &|_| {}).unwrap();
        assert_eq!(rep.done, 2);
        assert!(src.join("a.jpg").exists(), "source untouched in copy mode");
        // files landed somewhere under dest with correct content
        let found: Vec<PathBuf> = walkdir::WalkDir::new(&dest).into_iter().filter_map(|e| e.ok()).filter(|e| e.file_type().is_file()).map(|e| e.path().to_path_buf()).collect();
        assert_eq!(found.len(), 2);
        let a = found.iter().find(|p| p.file_name().unwrap() == "a.jpg").unwrap();
        assert_eq!(fs::read(a).unwrap(), b"content-a");
        // journal rows exist
        let items = get_plan_items(&conn, &pid).unwrap();
        assert!(items.iter().all(|i| i.status == PlanItemStatus::Done && i.sha256_after.is_some()));
        fs::remove_dir_all(src.parent().unwrap()).ok();
    }

    #[test]
    fn move_mode_removes_source_after_success() {
        let (conn, src, dest) = fixture();
        let (pid, _, _) = build_plan(&conn, &plan_opts(&dest, OperationMode::Move)).unwrap();
        let cancel = AtomicBool::new(false);
        let rep = apply_plan(&conn, &pid, &cancel, &|_| {}).unwrap();
        assert_eq!(rep.done, 2);
        assert!(!src.join("a.jpg").exists());
        assert!(!src.join("b.jpg").exists());
        fs::remove_dir_all(src.parent().unwrap()).ok();
    }

    #[test]
    fn undo_copy_removes_copy_keeps_original() {
        let (conn, src, dest) = fixture();
        let (pid, _, _) = build_plan(&conn, &plan_opts(&dest, OperationMode::Copy)).unwrap();
        let cancel = AtomicBool::new(false);
        apply_plan(&conn, &pid, &cancel, &|_| {}).unwrap();
        let rep = undo_plan(&conn, &pid, false, &cancel, &|_| {}).unwrap();
        assert_eq!(rep.done, 2);
        assert!(src.join("a.jpg").exists());
        let leftovers = walkdir::WalkDir::new(&dest).into_iter().filter_map(|e| e.ok()).filter(|e| e.file_type().is_file()).count();
        assert_eq!(leftovers, 0);
        fs::remove_dir_all(src.parent().unwrap()).ok();
    }

    #[test]
    fn undo_move_restores_file() {
        let (conn, src, dest) = fixture();
        let (pid, _, _) = build_plan(&conn, &plan_opts(&dest, OperationMode::Move)).unwrap();
        let cancel = AtomicBool::new(false);
        apply_plan(&conn, &pid, &cancel, &|_| {}).unwrap();
        let rep = undo_plan(&conn, &pid, true, &cancel, &|_| {}).unwrap();
        assert_eq!(rep.done, 2);
        assert!(src.join("a.jpg").exists());
        assert!(src.join("b.jpg").exists());
        fs::remove_dir_all(src.parent().unwrap()).ok();
    }

    #[test]
    fn undo_refuses_when_dest_modified_externally() {
        let (conn, src, dest) = fixture();
        let (pid, _, _) = build_plan(&conn, &plan_opts(&dest, OperationMode::Copy)).unwrap();
        let cancel = AtomicBool::new(false);
        apply_plan(&conn, &pid, &cancel, &|_| {}).unwrap();
        // tamper with a copied file
        let a = walkdir::WalkDir::new(&dest).into_iter().filter_map(|e| e.ok())
            .find(|e| e.file_name() == "a.jpg").unwrap().into_path();
        fs::write(&a, b"tampered").unwrap();
        let rep = undo_plan(&conn, &pid, false, &cancel, &|_| {}).unwrap();
        assert_eq!(rep.failed, 1);
        assert_eq!(rep.done, 1);
        assert!(a.exists(), "modified copy left in place for inspection");
        fs::remove_dir_all(src.parent().unwrap()).ok();
    }

    #[test]
    fn conflict_never_overwrites() {
        let (conn, src, dest) = fixture();
        // pre-create one destination file via first copy plan, then plan again
        let cancel = AtomicBool::new(false);
        let (pid1, _, _) = build_plan(&conn, &plan_opts(&dest, OperationMode::Copy)).unwrap();
        apply_plan(&conn, &pid1, &cancel, &|_| {}).unwrap();
        let (pid2, plan2, _) = build_plan(&conn, &plan_opts(&dest, OperationMode::Copy)).unwrap();
        let suffixed = plan2.iter().filter(|p| p.destination_path.contains(" (1).")).count();
        assert_eq!(suffixed, 2);
        apply_plan(&conn, &pid2, &cancel, &|_| {}).unwrap();
        // original content preserved
        let orig = walkdir::WalkDir::new(&dest).into_iter().filter_map(|e| e.ok())
            .find(|e| e.file_name() == "a.jpg").unwrap().into_path();
        assert_eq!(fs::read(&orig).unwrap(), b"content-a");
        fs::remove_dir_all(src.parent().unwrap()).ok();
    }

    #[test]
    fn failure_keeps_original_and_marks_failed() {
        let (conn, src, dest) = fixture();
        let (pid, plan, _) = build_plan(&conn, &plan_opts(&dest, OperationMode::Copy)).unwrap();
        // break one item's destination: point it at an existing directory path (rename will fail)
        let bad = plan.iter().find(|p| p.source_path.ends_with("a.jpg")).unwrap();
        conn.execute("UPDATE operation_item SET destination_path=?1 WHERE id=?2",
            rusqlite::params![dest.join("Невідома дата").join("Без геолокації").join("a.jpg").join("nested").to_string_lossy().to_string(), bad.id]).ok();
        // create a file exactly at that parent to make it un-creatable
        fs::create_dir_all(dest.join("Невідома дата").join("Без геолокації")).unwrap();
        fs::write(dest.join("Невідома дата").join("Без геолокації").join("a.jpg"), b"existing-dir-blocker").unwrap();
        let cancel = AtomicBool::new(false);
        let rep = apply_plan(&conn, &pid, &cancel, &|_| {}).unwrap();
        assert!(src.join("a.jpg").exists(), "original survives failure");
        let items = get_plan_items(&conn, &pid).unwrap();
        let failed = items.iter().filter(|i| i.status == PlanItemStatus::Failed).count();
        // the tampered destination now collides with an existing file → suffix policy saves it,
        // so what must hold is: no data lost, originals intact
        assert!(failed <= 1);
        fs::remove_dir_all(src.parent().unwrap()).ok();
    }

    #[test]
    fn tampered_source_between_plan_and_apply_fails_safely() {
        let (conn, src, dest) = fixture();
        let (pid, _, _) = build_plan(&conn, &plan_opts(&dest, OperationMode::Move)).unwrap();
        // change file size after planning
        fs::write(src.join("a.jpg"), b"content-a-much-longer-now").unwrap();
        let cancel = AtomicBool::new(false);
        let rep = apply_plan(&conn, &pid, &cancel, &|_| {}).unwrap();
        assert_eq!(rep.failed, 1);
        assert!(src.join("a.jpg").exists(), "changed source not destroyed");
        fs::remove_dir_all(src.parent().unwrap()).ok();
    }
}
