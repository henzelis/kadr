use crate::app_state::{err_str, AppState};
use kadr_core::domain::operation::ConflictPolicy;
use kadr_core::domain::sort_template::{MonthFormat, TemplateConfig};
use kadr_core::persistence::db;
use kadr_core::services::metadata;
use serde::{Deserialize, Serialize};
use tauri::State;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppSettings {
    pub destination_root: String,
    #[serde(default)]
    pub template: String,
    #[serde(default)]
    pub month_format: MonthFormat,
    #[serde(default)]
    pub conflict_policy: ConflictPolicy,
    pub unknown_date_label: Option<String>,
    pub no_geo_label: Option<String>,
    pub fs_date_fallback: bool,
    pub coords_as_label: bool,
    pub compute_hashes_on_scan: Option<bool>,
}

pub(crate) fn load_settings(conn: &rusqlite::Connection) -> AppSettings {
    match db::get_setting(conn, "settings") {
        Ok(Some(s)) => serde_json::from_str(&s).unwrap_or_default(),
        _ => AppSettings::default(),
    }
}

pub(crate) fn template_of(s: &AppSettings) -> TemplateConfig {
    TemplateConfig {
        template: if s.template.trim().is_empty() {
            kadr_core::domain::sort_template::DEFAULT_TEMPLATE.to_string()
        } else {
            s.template.clone()
        },
        unknown_date_label: s
            .unknown_date_label
            .clone()
            .unwrap_or_else(|| kadr_core::domain::sort_template::DEFAULT_UNKNOWN_DATE.to_string()),
        no_geo_label: s
            .no_geo_label
            .clone()
            .unwrap_or_else(|| kadr_core::domain::sort_template::DEFAULT_NO_GEO.to_string()),
        month_format: s.month_format,
        max_segment_len: 80,
        target_os: kadr_core::domain::sort_template::current_os(),
        coords_as_label: s.coords_as_label,
    }
}

#[tauri::command]
pub fn get_tool_status() -> metadata::ToolStatus {
    metadata::detect_tools()
}

#[tauri::command]
pub fn get_settings(state: State<AppState>) -> Result<AppSettings, String> {
    let conn = state.conn().map_err(err_str)?;
    Ok(load_settings(&conn))
}

#[tauri::command]
pub fn update_settings(state: State<AppState>, settings: AppSettings) -> Result<AppSettings, String> {
    // validate template eagerly so a broken pattern never reaches the planner
    let tc = template_of(&settings);
    kadr_core::domain::sort_template::parse_template(&tc.template)
        .map_err(|e| format!("Шаблон папок некоректний: {e}"))?;
    if !settings.destination_root.trim().is_empty() {
        let p = std::path::Path::new(&settings.destination_root);
        if !p.is_absolute() {
            return Err("Папка архіву має бути абсолютним шляхом".into());
        }
    }
    let conn = state.conn().map_err(err_str)?;
    db::set_setting(&conn, "settings", &serde_json::to_string(&settings).unwrap())
        .map_err(err_str)?;
    Ok(settings)
}

#[tauri::command]
pub fn add_source_root(state: State<AppState>, path: String) -> Result<db::SourceRoot, String> {
    let p = std::path::Path::new(&path);
    if !p.is_dir() {
        return Err(format!("«{path}» не є доступною папкою"));
    }
    let conn = state.conn().map_err(err_str)?;
    db::add_source_root(&conn, &path).map_err(err_str)
}

#[tauri::command]
pub fn remove_source_root(state: State<AppState>, id: String) -> Result<(), String> {
    let conn = state.conn().map_err(err_str)?;
    db::remove_source_root(&conn, &id).map_err(err_str)
}

#[tauri::command]
pub fn list_source_roots(state: State<AppState>) -> Result<Vec<db::SourceRoot>, String> {
    let conn = state.conn().map_err(err_str)?;
    db::list_source_roots(&conn).map_err(err_str)
}

#[tauri::command]
pub fn get_logs(state: State<AppState>, limit: Option<i64>) -> Result<Vec<kadr_core::domain::operation::OperationLogEntry>, String> {
    let conn = state.conn().map_err(err_str)?;
    db::recent_logs(&conn, limit.unwrap_or(100)).map_err(err_str)
}
