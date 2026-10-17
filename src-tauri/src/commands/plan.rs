use crate::app_state::{err_str, AppState};
use crate::commands::settings::{load_settings, template_of};
use kadr_core::domain::operation::{OperationMode, PlanItem, PlanSummary};
use kadr_core::persistence::db;
use kadr_core::services::path_planner::{self, PlanOptions};
use serde::Serialize;
use tauri::State;

#[derive(Serialize)]
pub struct BuiltPlan {
    pub plan_id: String,
    pub summary: PlanSummary,
    pub items: Vec<PlanItem>,
}

#[derive(serde::Serialize)]
pub struct UnfinishedSort {
    pub plan_id: String,
    pub mode: String,
    pub state: String,
    pub done: i64,
    pub failed: i64,
    pub total: i64,
}

/// Recover-after-crash entry point: is there a half-applied sort waiting?
#[tauri::command]
pub fn get_unfinished_sort(state: State<AppState>) -> Result<Option<UnfinishedSort>, String> {
    let conn = state.conn().map_err(err_str)?;
    let Some((id, mode, st)) = db::last_unfinished_plan(&conn).map_err(err_str)? else {
        return Ok(None);
    };
    let (done, failed, total) = conn
        .query_row(
            "SELECT COALESCE(SUM(status='done'),0), COALESCE(SUM(status='failed'),0), COUNT(*)
             FROM operation_item WHERE plan_id=?1",
            rusqlite::params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap_or((0, 0, 0));
    Ok(Some(UnfinishedSort {
        plan_id: id,
        mode,
        state: st,
        done,
        failed,
        total,
    }))
}

#[tauri::command]
pub fn build_sort_plan(state: State<AppState>, mode: Option<String>) -> Result<BuiltPlan, String> {
    if state.op_running.load(std::sync::atomic::Ordering::Relaxed) {
        return Err("Файлова операція вже виконується".into());
    }
    let conn = state.conn().map_err(err_str)?;
    let settings = load_settings(&conn);
    if settings.destination_root.trim().is_empty() {
        return Err("Спершу виберіть папку архіву в Налаштуваннях".into());
    }
    let opts = PlanOptions {
        destination_root: settings.destination_root.clone(),
        template: template_of(&settings),
        mode: match mode.as_deref() {
            Some("move") => OperationMode::Move,
            _ => OperationMode::Copy,
        },
        conflict: settings.conflict_policy,
        media_ids: None,
    };
    let (plan_id, items, summary) = path_planner::build_plan(&conn, &opts).map_err(err_str)?;
    Ok(BuiltPlan {
        plan_id,
        summary,
        items,
    })
}

#[tauri::command]
pub fn get_sort_plan(
    state: State<AppState>,
    plan_id: String,
) -> Result<(Vec<PlanItem>, PlanSummary), String> {
    let conn = state.conn().map_err(err_str)?;
    let items = db::get_plan_items(&conn, &plan_id).map_err(err_str)?;
    let mut summary = PlanSummary::default();
    for it in &items {
        summary.total += 1;
        match it.mode {
            OperationMode::Copy => summary.to_copy += 1,
            OperationMode::Move => summary.to_move += 1,
        }
        match it.status {
            kadr_core::domain::operation::PlanItemStatus::Skipped => summary.skipped += 1,
            kadr_core::domain::operation::PlanItemStatus::NeedsUser => summary.conflicts += 1,
            _ => {}
        }
        if it.warnings.iter().any(|w| w.code == "unknown_date") {
            summary.unknown_date += 1;
        }
        if it.warnings.iter().any(|w| w.code == "unknown_place") {
            summary.unknown_place += 1;
        }
        summary.expected_bytes += it.size_bytes;
    }
    Ok((items, summary))
}

#[tauri::command]
pub fn update_plan_item(
    state: State<AppState>,
    plan_id: String,
    item_id: String,
    place: Option<String>,
    date: Option<String>,
) -> Result<String, String> {
    let conn = state.conn().map_err(err_str)?;
    path_planner::recompute_item_dest(&conn, &plan_id, &item_id, place.as_deref(), date.as_deref())
        .map_err(err_str)
}

#[tauri::command]
pub fn exclude_plan_item(
    state: State<AppState>,
    item_id: String,
    excluded: bool,
) -> Result<(), String> {
    let conn = state.conn().map_err(err_str)?;
    let status = if excluded {
        kadr_core::domain::operation::PlanItemStatus::Excluded
    } else {
        kadr_core::domain::operation::PlanItemStatus::Pending
    };
    db::update_plan_item_status(&conn, &item_id, status).map_err(err_str)
}

#[tauri::command]
pub fn set_plan_mode(state: State<AppState>, plan_id: String, mode: String) -> Result<(), String> {
    let conn = state.conn().map_err(err_str)?;
    let m = match mode.as_str() {
        "move" => OperationMode::Move,
        _ => OperationMode::Copy,
    };
    db::update_plan_item_mode(&conn, &plan_id, m).map_err(err_str)
}
