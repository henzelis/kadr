use crate::app_state::{err_str, AppState};
use crate::commands::settings::load_settings;
use kadr_core::persistence::db;
use kadr_core::services::scanner::{self, ScanOptions};
use serde::Serialize;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Emitter, State};

#[derive(Serialize, Clone)]
pub struct ScanProgressPayload {
    pub job_id: String,
    pub found: i64,
    pub processed: i64,
    pub skipped: i64,
    pub errors: i64,
    pub total: i64,
    pub elapsed_ms: u64,
    pub current: String,
    pub finished: bool,
    pub cancelled: bool,
    pub error: Option<String>,
}

#[tauri::command]
pub fn start_scan(
    app: AppHandle,
    state: State<AppState>,
    root_id: Option<String>,
) -> Result<String, String> {
    if state.scan_running.load(Ordering::Relaxed) {
        return Err("Сканування вже виконується".into());
    }
    let conn = state.conn().map_err(err_str)?;
    let roots = db::list_source_roots(&conn).map_err(err_str)?;
    if roots.is_empty() {
        return Err("Додайте вихідну папку в Налаштуваннях, перш ніж сканувати".into());
    }
    // Without an explicit root, every added source is scanned one after
    // another — users add several folders and expect all of them indexed.
    let roots_to_scan: Vec<_> = match root_id {
        Some(id) => vec![roots
            .into_iter()
            .find(|r| r.id == id)
            .ok_or("Джерело не знайдено")?],
        None => roots,
    };
    let settings = load_settings(&conn);
    let opts = ScanOptions {
        fs_date_fallback: settings.fs_date_fallback,
        compute_hashes: settings.compute_hashes_on_scan.unwrap_or(false),
        ..Default::default()
    };
    drop(conn);

    let job_id = uuid::Uuid::new_v4().to_string();
    state.scan_running.store(true, Ordering::Relaxed);
    state.scan_cancel.store(false, Ordering::Relaxed);

    let db_path = state.db_path.clone();
    let cancel = state.scan_cancel.clone();
    let running = state.scan_running.clone();
    let jid = job_id.clone();

    std::thread::spawn(move || {
        let t0 = std::time::Instant::now();
        let emit = |found: i64,
                    processed: i64,
                    skipped: i64,
                    errors: i64,
                    current: &str,
                    finished: bool,
                    cancelled: bool,
                    total: i64,
                    error: Option<String>| {
            let _ = app.emit(
                "scan-progress",
                ScanProgressPayload {
                    job_id: jid.clone(),
                    found,
                    processed,
                    skipped,
                    errors,
                    total,
                    elapsed_ms: t0.elapsed().as_millis() as u64,
                    current: current.to_string(),
                    finished,
                    cancelled,
                    error,
                },
            );
        };
        match db::open_db(&db_path.to_string_lossy()) {
            Ok(conn) => {
                // Cheap pre-count so the UI can draw a determinate bar with ETA.
                let total: i64 = roots_to_scan
                    .iter()
                    .map(|r| scanner::count_media(std::path::Path::new(&r.path), &opts))
                    .sum();
                emit(0, 0, 0, 0, "підрахунок файлів…", false, false, total, None);
                let mut totals = scanner::ScanProgress::default();
                let mut problems: Vec<String> = Vec::new();
                for root in &roots_to_scan {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    // Each source root gets its own journal job row.
                    let root_job = uuid::Uuid::new_v4().to_string();
                    let base = totals.clone();
                    let result =
                        scanner::scan_root(&conn, root, &opts, &root_job, cancel.as_ref(), &|p| {
                            let mut t = base.clone();
                            t.found += p.found;
                            t.processed += p.processed;
                            t.skipped += p.skipped;
                            t.errors += p.errors;
                            emit(
                                t.found,
                                t.processed,
                                t.skipped,
                                t.errors,
                                &p.current,
                                false,
                                false,
                                total,
                                None,
                            );
                        });
                    match result {
                        Ok(p) => {
                            totals.found += p.found;
                            totals.processed += p.processed;
                            totals.skipped += p.skipped;
                            totals.errors += p.errors;
                        }
                        // One unreachable folder must not stop the other scans.
                        Err(e) => problems.push(format!("{}: {}", root.path, e.friendly())),
                    }
                }
                let error = if problems.is_empty() {
                    None
                } else {
                    Some(problems.join(" · "))
                };
                emit(
                    totals.found,
                    totals.processed,
                    totals.skipped,
                    totals.errors,
                    "",
                    true,
                    cancel.load(Ordering::Relaxed),
                    total,
                    error,
                );
            }
            Err(e) => emit(0, 0, 0, 0, "", true, false, 0, Some(e.friendly())),
        }
        running.store(false, Ordering::Relaxed);
    });
    Ok(job_id)
}

#[tauri::command]
pub fn cancel_scan(state: State<AppState>) -> Result<(), String> {
    state.scan_cancel.store(true, Ordering::Relaxed);
    Ok(())
}

#[tauri::command]
pub fn scan_status(state: State<AppState>) -> Result<bool, String> {
    Ok(state.scan_running.load(Ordering::Relaxed))
}
