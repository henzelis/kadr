//! CSV/JSON report export with actual metadata, destinations and statuses (§18.20).

use crate::persistence::db;
use crate::errors::{KResult, KadrError};
use rusqlite::Connection;
use std::io::Write;
use std::path::Path;

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains(';') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

pub fn export_plan_csv(conn: &Connection, plan_id: &str, out: &Path) -> KResult<usize> {
    let items = db::get_plan_items(conn, plan_id)?;
    let mut f = std::fs::File::create(out).map_err(|e| KadrError::Io {
        path: out.display().to_string(),
        source: e,
    })?;
    let mut w = std::io::BufWriter::new(&mut f);
    writeln!(
        w,
        "file;source_path;destination_path;mode;status;size_bytes;captured_at;date_source;place;location_source;sha256;warnings;error"
    )
    .map_err(|e| KadrError::Io { path: out.display().to_string(), source: e })?;
    for it in &items {
        let media = db::get_media_item(conn, &it.media_id)?;
        let (cap, dsrc, place, lsrc) = match &media {
            Some(m) => (
                m.captured_at_local.clone().unwrap_or_else(|| "—".into()),
                m.date_source.map(db::date_source_str).unwrap_or("none").to_string(),
                m.place_label.clone().unwrap_or_else(|| "—".into()),
                m.location_source.map(db::location_source_str).unwrap_or("none").to_string(),
            ),
            None => ("—".into(), "none".into(), "—".into(), "none".into()),
        };
        let warnings = it
            .warnings
            .iter()
            .map(|w| w.code.clone())
            .collect::<Vec<_>>()
            .join("|");
        writeln!(
            w,
            "{}",
            [
                csv_escape(&media.map(|m| m.file_name).unwrap_or_default()),
                csv_escape(&it.source_path),
                csv_escape(&it.destination_path),
                if it.mode == crate::domain::operation::OperationMode::Copy { "copy".into() } else { "move".into() },
                db::plan_status_str(it.status).to_string(),
                it.size_bytes.to_string(),
                csv_escape(&cap),
                dsrc,
                csv_escape(&place),
                lsrc,
                it.sha256_before.clone().unwrap_or_default(),
                warnings,
                csv_escape(it.error.as_deref().unwrap_or("")),
            ]
            .join(";")
        )
        .map_err(|e| KadrError::Io { path: out.display().to_string(), source: e })?;
    }
    w.flush().ok();
    Ok(items.len())
}

pub fn export_plan_json(conn: &Connection, plan_id: &str, out: &Path) -> KResult<usize> {
    let items = db::get_plan_items(conn, plan_id)?;
    let value = serde_json::to_string_pretty(&items).map_err(|e| KadrError::user_detail("JSON export", e.to_string()))?;
    std::fs::write(out, value).map_err(|e| KadrError::Io { path: out.display().to_string(), source: e })?;
    Ok(items.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::operation::ConflictPolicy;
    use crate::domain::sort_template::{TargetOs, TemplateConfig};
    use crate::persistence::db::{add_source_root, open_db};
    use crate::services::path_planner::{build_plan, PlanOptions};
    use crate::services::scanner::{scan_root, ScanOptions};
    use std::sync::atomic::AtomicBool;

    #[test]
    fn csv_has_real_paths_and_statuses() {
        let dir = std::env::temp_dir().join(format!("kadr-rep-{}", uuid::Uuid::new_v4()));
        let src = dir.join("src");
        let dest = dir.join("dest");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(src.join("x.jpg"), b"x").unwrap();
        let conn = open_db(":memory:").unwrap();
        let root = add_source_root(&conn, src.to_str().unwrap()).unwrap();
        let cancel = AtomicBool::new(false);
        scan_root(&conn, &root, &ScanOptions::default(), "j", &cancel, &|_| {}).unwrap();
        let (pid, _, _) = build_plan(
            &conn,
            &PlanOptions {
                destination_root: dest.to_string_lossy().to_string(),
                template: TemplateConfig { target_os: TargetOs::Unix, ..Default::default() },
                mode: crate::domain::operation::OperationMode::Copy,
                conflict: ConflictPolicy::Suffix,
                media_ids: None,
            },
        )
        .unwrap();
        crate::services::operation_runner::apply_plan(&conn, &pid, &cancel, &|_| {}).unwrap();
        let csv_path = dir.join("report.csv");
        let n = export_plan_csv(&conn, &pid, &csv_path).unwrap();
        assert_eq!(n, 1);
        let text = std::fs::read_to_string(&csv_path).unwrap();
        assert!(text.contains("x.jpg"));
        assert!(text.contains(";done;"));
        let json_path = dir.join("report.json");
        export_plan_json(&conn, &pid, &json_path).unwrap();
        let j: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&json_path).unwrap()).unwrap();
        assert_eq!(j.as_array().unwrap()[0]["status"], "done");
        std::fs::remove_dir_all(&dir).ok();
    }
}
