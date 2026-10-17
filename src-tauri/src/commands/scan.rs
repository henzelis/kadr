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
    let root = match root_id {
        Some(id) => roots
            .into_iter()
            .find(|r| r.id == id)
            .ok_or("Джерело не знайдено")?,
        None => roots.into_iter().next().unwrap(),
    };
    let settings = load_settings(&conn);
    let opts = ScanOptions {
        fs_date_fallback: settings.fs_date_fallback,
        compute_hashes: settings.compute_hashes_on_scan.unwrap_or(true),
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
        let emit = |found: i64,
                    processed: i64,
                    skipped: i64,
                    errors: i64,
                    current: &str,
                    finished: bool,
                    error: Option<String>| {
            let _ = app.emit(
                "scan-progress",
                ScanProgressPayload {
                    job_id: jid.clone(),
                    found,
                    processed,
                    skipped,
                    errors,
                    current: current.to_string(),
                    finished,
                    cancelled: false,
                    error,
                },
            );
        };
        match db::open_db(&db_path.to_string_lossy()) {
            Ok(conn) => {
                let result = scanner::scan_root(&conn, &root, &opts, &jid, cancel.as_ref(), &|p| {
                    emit(
                        p.found,
                        p.processed,
                        p.skipped,
                        p.errors,
                        &p.current,
                        false,
                        None,
                    )
                });
                match result {
                    Ok(p) => {
                        let cancelled = cancel.load(Ordering::Relaxed);
                        emit(p.found, p.processed, p.skipped, p.errors, "", true, None);
                        let _ = app.emit(
                            "scan-progress",
                            ScanProgressPayload {
                                job_id: jid,
                                found: p.found,
                                processed: p.processed,
                                skipped: p.skipped,
                                errors: p.errors,
                                current: String::new(),
                                finished: true,
                                cancelled,
                                error: None,
                            },
                        );
                    }
                    Err(e) => emit(0, 0, 0, 0, "", true, Some(e.friendly())),
                }
            }
            Err(e) => emit(0, 0, 0, 0, "", true, Some(e.friendly())),
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
