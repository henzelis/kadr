use crate::app_state::{err_str, AppState};
use kadr_core::services::{duplicate, report};
use serde::Serialize;
use tauri::State;

#[derive(Serialize)]
pub struct LibraryStats {
    pub total: i64,
    pub photos: i64,
    pub videos: i64,
    pub unknown_date: i64,
    pub no_geo: i64,
    pub coords_only: i64,
    pub needs_review: i64,
    pub duplicate_groups: i64,
    pub total_bytes: i64,
}

#[tauri::command]
pub fn library_stats(state: State<AppState>) -> Result<LibraryStats, String> {
    let conn = state.conn().map_err(err_str)?;
    let q = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap_or(0) };
    Ok(LibraryStats {
        total: q("SELECT COUNT(*) FROM media_item WHERE media_kind != 'sidecar'"),
        photos: q("SELECT COUNT(*) FROM media_item WHERE media_kind = 'photo'"),
        videos: q("SELECT COUNT(*) FROM media_item WHERE media_kind = 'video'"),
        unknown_date: q("SELECT COUNT(*) FROM media_item WHERE media_kind != 'sidecar' AND date_status = 'unknown'"),
        no_geo: q("SELECT COUNT(*) FROM media_item WHERE media_kind != 'sidecar' AND place_status = 'no_geo'"),
        coords_only: q("SELECT COUNT(*) FROM media_item WHERE media_kind != 'sidecar' AND place_status = 'coords_only'"),
        needs_review: q("SELECT COUNT(*) FROM media_item WHERE date_status='needs_review' OR place_status='needs_review' OR metadata_status='needs_review'"),
        duplicate_groups: duplicate::hash_group_count(&conn).unwrap_or(0),
        total_bytes: q("SELECT COALESCE(SUM(size_bytes),0) FROM media_item WHERE media_kind != 'sidecar'"),
    })
}

#[tauri::command]
pub fn export_report(
    state: State<AppState>,
    plan_id: String,
    out_path: String,
    format: String,
) -> Result<usize, String> {
    let conn = state.conn().map_err(err_str)?;
    let path = std::path::Path::new(&out_path);
    if !path.parent().map(|p| p.exists()).unwrap_or(false) {
        return Err("Папка для звіту не існує".into());
    }
    match format.as_str() {
        "json" => report::export_plan_json(&conn, &plan_id, path).map_err(err_str),
        _ => report::export_plan_csv(&conn, &plan_id, path).map_err(err_str),
    }
}
