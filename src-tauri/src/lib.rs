mod app_state;
mod commands;

use app_state::AppState;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            if let Ok(rd) = app.path().resource_dir() {
                std::env::set_var("KADR_HELPER_DIR", rd.join("helpers"));
                let geo = rd.join("resources").join("geobase.tsv");
                let geo = if geo.exists() {
                    geo
                } else {
                    rd.join("geobase.tsv")
                };
                std::env::set_var("KADR_GEOBASE", geo);
            }
            let data_dir = app.path().app_data_dir().expect("app data dir");
            std::fs::create_dir_all(&data_dir).ok();
            let state = AppState::new(&data_dir);
            // initialize the database schema once at startup
            let conn = kadr_core::persistence::db::open_db(&state.db_path.to_string_lossy())
                .map_err(|e| e.friendly().to_string())?;
            drop(conn);
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::settings::get_tool_status,
            commands::settings::get_settings,
            commands::settings::update_settings,
            commands::settings::add_source_root,
            commands::settings::remove_source_root,
            commands::settings::list_source_roots,
            commands::settings::get_logs,
            commands::scan::start_scan,
            commands::scan::cancel_scan,
            commands::scan::scan_status,
            commands::library::query_media,
            commands::library::get_media_details,
            commands::library::get_thumbnail_base64,
            commands::library::duplicate_groups,
            commands::library::ensure_hashes,
            commands::library::generate_thumbnails,
            commands::metadata::set_manual_place,
            commands::metadata::set_date_override,
            commands::plan::build_sort_plan,
            commands::plan::get_sort_plan,
            commands::plan::update_plan_item,
            commands::plan::exclude_plan_item,
            commands::plan::set_plan_mode,
            commands::operations::apply_sort_plan,
            commands::operations::cancel_operation,
            commands::operations::undo_operation,
            commands::operations::retry_failed_items,
            commands::report::export_report,
            commands::report::library_stats,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Kadr");
}
