use crate::app_state::{err_str, AppState};
use kadr_core::persistence::db;
use tauri::State;

#[tauri::command]
pub fn set_manual_place(state: State<AppState>, media_ids: Vec<String>, place: String) -> Result<(), String> {
    let conn = state.conn().map_err(err_str)?;
    db::set_manual_place(&conn, &media_ids, &place).map_err(err_str)
}

#[tauri::command]
pub fn set_date_override(state: State<AppState>, id: String, date: String) -> Result<(), String> {
    let conn = state.conn().map_err(err_str)?;
    db::set_manual_date(&conn, &id, &date).map_err(err_str)
}
