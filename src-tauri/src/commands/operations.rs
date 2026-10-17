use crate::app_state::{err_str, AppState};
use kadr_core::persistence::db;
use kadr_core::services::operation_runner;
use serde::Serialize;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Emitter, State};

#[derive(Serialize, Clone)]
pub struct ApplyPayload {
    pub plan_id: String,
    pub total: usize,
    pub done: usize,
    pub failed: usize,
    pub skipped: usize,
    pub current: String,
    pub finished: bool,
    pub cancelled: bool,
    pub error: Option<String>,
}

#[tauri::command]
pub fn apply_sort_plan(
    app: AppHandle,
    state: State<AppState>,
    plan_id: String,
    confirmation: bool,
) -> Result<(), String> {
    // §11: never touch files without explicit confirmation
    if !confirmation {
        return Err("Потрібне явне підтвердження застосування плану".into());
    }
    if state.op_running.load(Ordering::Relaxed) {
        return Err("Інша файлова операція вже виконується".into());
    }
    let (meta, items) = {
        let conn = state.conn().map_err(err_str)?;
        let meta = db::get_plan_meta(&conn, &plan_id)
            .map_err(err_str)?
            .ok_or("План не знайдено")?;
        let items = db::get_plan_items(&conn, &plan_id).map_err(err_str)?;
        (meta, items)
    };
    if meta.2 == "applying" || meta.2 == "applied" || meta.2 == "undone" {
        return Err("Цей план уже застосовано".into());
    }
    let pending = items.iter().filter(|i| matches!(i.status, kadr_core::domain::operation::PlanItemStatus::Pending)).count();
    if pending == 0 {
        return Err("У плані немає файлів до виконання".into());
    }

    state.op_running.store(true, Ordering::Relaxed);
    state.op_cancel.store(false, Ordering::Relaxed);
    let db_path = state.db_path.clone();
    let cancel = state.op_cancel.clone();
    let running = state.op_running.clone();

    std::thread::spawn(move || {
        let emit = |total: usize, done: usize, failed: usize, skipped: usize, current: &str, finished: bool, error: Option<String>| {
            let _ = app.emit("apply-progress", ApplyPayload {
                plan_id: plan_id.clone(), total, done, failed, skipped,
                current: current.to_string(), finished, cancelled: false, error,
            });
        };
        emit(items.len(), 0, 0, 0, "", false, None);
        match db::open_db(&db_path.to_string_lossy()) {
            Ok(conn) => {
                let res = operation_runner::apply_plan(
                    &conn,
                    &plan_id,
                    cancel.as_ref(),
                    &|p| emit(p.total, p.done, p.failed, p.skipped, &p.current, false, None),
                );
                match res {
                    Ok(r) => {
                        let _ = app.emit("apply-progress", ApplyPayload {
                            plan_id, total: items.len(), done: r.done, failed: r.failed, skipped: r.skipped,
                            current: String::new(), finished: true, cancelled: r.cancelled, error: None,
                        });
                    }
                    Err(e) => emit(items.len(), 0, 0, 0, "", true, Some(e.friendly())),
                }
            }
            Err(e) => emit(items.len(), 0, 0, 0, "", true, Some(e.friendly())),
        }
        running.store(false, Ordering::Relaxed);
    });
    Ok(())
}

#[tauri::command]
pub fn cancel_operation(state: State<AppState>) -> Result<(), String> {
    state.op_cancel.store(true, Ordering::Relaxed);
    Ok(())
}

#[tauri::command]
pub fn undo_operation(
    app: AppHandle,
    state: State<AppState>,
    plan_id: String,
) -> Result<(), String> {
    if state.op_running.load(Ordering::Relaxed) {
        return Err("Зачекайте завершення поточної операції".into());
    }
    let (mode, items) = {
        let conn = state.conn().map_err(err_str)?;
        let meta = db::get_plan_meta(&conn, &plan_id)
            .map_err(err_str)?
            .ok_or("План не знайдено")?;
        let items = db::get_plan_items(&conn, &plan_id).map_err(err_str)?;
        (meta.0 == "move", items)
    };
    state.op_running.store(true, Ordering::Relaxed);
    state.op_cancel.store(false, Ordering::Relaxed);
    let db_path = state.db_path.clone();
    let cancel = state.op_cancel.clone();
    let running = state.op_running.clone();

    std::thread::spawn(move || {
        match db::open_db(&db_path.to_string_lossy()) {
            Ok(conn) => {
                let res = operation_runner::undo_plan(
                    &conn,
                    &plan_id,
                    mode,
                    cancel.as_ref(),
                    &|p| {
                        let _ = app.emit("apply-progress", ApplyPayload {
                            plan_id: plan_id.clone(), total: p.total, done: p.done, failed: p.failed,
                            skipped: p.skipped, current: p.current.clone(), finished: false, cancelled: false, error: None,
                        });
                    },
                );
                match res {
                    Ok(r) => {
                        let _ = app.emit("apply-progress", ApplyPayload {
                            plan_id, total: items.len(), done: r.done, failed: r.failed, skipped: r.skipped,
                            current: String::new(), finished: true, cancelled: r.cancelled, error: None,
                        });
                    }
                    Err(e) => {
                        let _ = app.emit("apply-progress", ApplyPayload {
                            plan_id: plan_id.clone(), total: 0, done: 0, failed: 0, skipped: 0,
                            current: String::new(), finished: true, cancelled: false, error: Some(e.friendly()),
                        });
                    }
                }
            }
            Err(e) => {
                let _ = app.emit("apply-progress", ApplyPayload {
                    plan_id, total: 0, done: 0, failed: 0, skipped: 0,
                    current: String::new(), finished: true, cancelled: false, error: Some(e.friendly()),
                });
            }
        }
        running.store(false, Ordering::Relaxed);
    });
    Ok(())
}

#[tauri::command]
pub fn retry_failed_items(state: State<AppState>, plan_id: String) -> Result<usize, String> {
    let conn = state.conn().map_err(err_str)?;
    operation_runner::retry_failed(&conn, &plan_id).map_err(err_str)
}
