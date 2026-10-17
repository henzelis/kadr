//! Deterministic plan builder — computes source→destination mappings with
//! zero filesystem modifications (TECHNICAL_SPEC §11 Plan, §8 rules).

use crate::domain::captured_date::DateStatus;
use crate::domain::operation::{ConflictPolicy, OperationMode, PlanItem, PlanItemStatus, PlanSummary, PlanWarning};
use crate::domain::sort_template::{build_dest_segments, TemplateConfig};
use crate::errors::{KResult, KadrError};
use chrono::{Datelike, NaiveDateTime};
use rusqlite::Connection;
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone)]
pub struct PlanOptions {
    pub destination_root: String,
    pub template: TemplateConfig,
    pub mode: OperationMode,
    pub conflict: ConflictPolicy,
    /// Restrict to specific media ids; None = whole library (photos/videos).
    pub media_ids: Option<Vec<String>>,
}

/// Reject destinations that escape the archive root or contain traversal.
pub fn is_inside_root(root: &str, dest: &Path) -> bool {
    let dest_norm = crate::services::scanner::normalize_path(dest);
    let root_norm = crate::services::scanner::normalize_path(Path::new(root));
    dest_norm == root_norm || dest_norm.starts_with(&format!("{root_norm}{}", std::path::MAIN_SEPARATOR))
}

fn path_is_safe_abs(p: &Path) -> bool {
    p.is_absolute() && !p.components().any(|c| matches!(c, Component::ParentDir))
}

fn parse_local(s: Option<&String>) -> Option<NaiveDateTime> {
    let s = s?;
    NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").ok()
}

pub fn build_plan(conn: &Connection, opts: &PlanOptions) -> KResult<(String, Vec<PlanItem>, PlanSummary)> {
    if opts.destination_root.trim().is_empty() {
        return Err(KadrError::user("Не вибрано папку архіву. Відкрийте Налаштування та вкажіть цільову теку."));
    }
    let dest_root = Path::new(&opts.destination_root);
    if !path_is_safe_abs(dest_root) {
        return Err(KadrError::user("Шлях архіву має бути абсолютним без '..'"));
    }
    // refuse sorting into an indexed source root (would duplicate on rescan)
    let roots: Vec<String> = conn
        .prepare("SELECT path FROM source_root")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;

    let plan_id = uuid::Uuid::new_v4().to_string();

    let items: Vec<crate::domain::media_item::MediaItem> = match &opts.media_ids {
        Some(ids) if !ids.is_empty() => {
            let mut out = Vec::new();
            let mut stmt = conn.prepare(&format!(
                "SELECT {} FROM media_item WHERE id IN ({})",
                crate::persistence::db::MEDIA_COLS,
                ids.iter().map(|_| "?").collect::<Vec<_>>().join(",")
            ))?;
            let mut mapped = stmt.query_map(rusqlite::params_from_iter(ids.iter()), crate::persistence::db::row_to_media)?;
            while let Some(r) = mapped.next() {
                out.push(r?);
            }
            out
        }
        _ => {
            let mut stmt = conn.prepare(&format!(
                "SELECT {} FROM media_item WHERE media_kind != 'sidecar'",
                crate::persistence::db::MEDIA_COLS
            ))?;
            let mut out = Vec::new();
            let mut mapped = stmt.query_map([], crate::persistence::db::row_to_media)?;
            while let Some(r) = mapped.next() {
                out.push(r?);
            }
            out
        }
    };

    let mut plan_items: Vec<PlanItem> = Vec::new();
    let mut used_dests: HashSet<String> = HashSet::new();
    let mut summary = PlanSummary::default();

    for m in &items {
        let mut warnings: Vec<PlanWarning> = Vec::new();

        // same-path source/destination guard
        let norm_src = crate::services::scanner::normalize_path(Path::new(&m.path));
        let norm_dest_root = crate::services::scanner::normalize_path(dest_root);
        if norm_src == norm_dest_root {
            return Err(KadrError::user(
                "Папка архіву збігається з індексованим джерелом. Виберіть іншу теку.",
            ));
        }
        if norm_src.starts_with(&format!("{norm_dest_root}{}", std::path::MAIN_SEPARATOR)) {
            warnings.push(PlanWarning {
                code: "source_inside_dest".into(),
                message: "Джерело вже лежить у папці архіву — копія створиться поруч.".into(),
            });
        }
        for r in &roots {
            let rn = crate::services::scanner::normalize_path(Path::new(r));
            if rn == norm_dest_root {
                return Err(KadrError::user(format!(
                    "Папка архіву збігається з індексованим джерелом «{r}». Виберіть іншу теку."
                )));
            }
            if norm_dest_root.starts_with(&format!("{rn}{}", std::path::MAIN_SEPARATOR)) {
                warnings.push(PlanWarning {
                    code: "dest_inside_source".into(),
                    message: format!("Архів лежить всередині індексованої папки «{r}» — наступне сканування побачить копії."),
                });
                break;
            }
        }

        let date_ok = matches!(m.date_status, DateStatus::Confirmed | DateStatus::Fallback);
        let local = parse_local(m.captured_at_local.as_ref());
        let year_month = match (date_ok, local) {
            (true, Some(dt)) => Some((dt.year(), dt.month())),
            _ => {
                warnings.push(PlanWarning {
                    code: "unknown_date".into(),
                    message: match m.date_status {
                        DateStatus::NeedsReview => "Дата суперечлива — потрібна перевірка".into(),
                        DateStatus::Fallback => "Дата взята з файлової системи (не підтверджена)".into(),
                        _ => "Надійну дату не знайдено".into(),
                    },
                });
                None
            }
        };

        let place = m.place_label.as_deref().filter(|s| !s.trim().is_empty());
        if place.is_none() && m.latitude.is_none() {
            warnings.push(PlanWarning {
                code: "unknown_place".into(),
                message: "Місце не визначено".into(),
            });
        }
        if m.date_status == DateStatus::NeedsReview || m.place_status == crate::domain::location::PlaceStatus::NeedsReview {
            warnings.push(PlanWarning {
                code: "needs_review".into(),
                message: "Частина метаданих потребує перевірки".into(),
            });
        }

        let coords_label = match (m.latitude, m.longitude) {
            (Some(la), Some(lo)) => Some(format!("{la:.5}, {lo:.5}")),
            _ => None,
        };

        let segs = build_dest_segments(
            &opts.template,
            &crate::domain::sort_template::PathInput {
                year_month,
                place,
                coords_label: coords_label.as_deref(),
                filename: &m.file_name,
            },
        );
        let mut dest: PathBuf = dest_root.to_path_buf();
        for s in &segs {
            dest.push(s);
        }
        if !is_inside_root(&opts.destination_root, &dest) || !path_is_safe_abs(&dest) {
            return Err(KadrError::user_detail(
                "Заблоковано спробу вийти за межі папки архіву",
                m.path.clone(),
            ));
        }

        let dest_str = dest.to_string_lossy().to_string();
        let mut status = PlanItemStatus::Pending;
        let final_dest = if used_dests.contains(&dest_str) || Path::new(&dest_str).exists() {
            match opts.conflict {
                ConflictPolicy::Suffix => {
                    let used = used_dests.clone();
                    let unique = crate::domain::operation::unique_name(&dest_str, |p| {
                        used.contains(p) || Path::new(p).exists()
                    });
                    warnings.push(PlanWarning {
                        code: "conflict".into(),
                        message: "Назву файлу адаптировано суфіксом, щоб нічого не перезаписати".into(),
                    });
                    summary.conflicts += 1;
                    unique
                }
                ConflictPolicy::Skip => {
                    warnings.push(PlanWarning {
                        code: "conflict".into(),
                        message: "Конфлікт назви — файл пропущено (політика «Пропустити») ".into(),
                    });
                    summary.conflicts += 1;
                    status = PlanItemStatus::Skipped;
                    dest_str.clone()
                }
                ConflictPolicy::Ask => {
                    warnings.push(PlanWarning {
                        code: "conflict".into(),
                        message: "Конфлікт назви — потрібне рішення користувача".into(),
                    });
                    summary.conflicts += 1;
                    status = PlanItemStatus::NeedsUser;
                    dest_str.clone()
                }
            }
        } else {
            dest_str
        };

        used_dests.insert(final_dest.clone());
        let pi = PlanItem {
            id: uuid::Uuid::new_v4().to_string(),
            plan_id: plan_id.clone(),
            media_id: m.id.clone(),
            source_path: m.path.clone(),
            destination_path: final_dest,
            mode: opts.mode,
            status,
            size_bytes: m.size_bytes,
            sha256_before: m.sha256.clone(),
            sha256_after: None,
            warnings,
            sidecar_of: None,
            place_override: None,
            error: None,
            finished_at: None,
            undo_source_path: None,
            undo_dest_path: None,
        };
        match opts.mode {
            OperationMode::Copy => summary.to_copy += 1,
            OperationMode::Move => summary.to_move += 1,
        }
        if year_month.is_none() {
            summary.unknown_date += 1;
        }
        if place.is_none() {
            summary.unknown_place += 1;
        }
        summary.expected_bytes += m.size_bytes;
        summary.total += 1;
        plan_items.push(pi);

        // sidecars travel with the parent item
        let parent_pi_id = plan_items.last().unwrap().id.clone();
        let children: Vec<crate::domain::media_item::MediaItem> = {
            let mut stmt = conn.prepare(&format!(
                "SELECT {} FROM media_item WHERE sidecar_parent_id = ?1",
                crate::persistence::db::MEDIA_COLS
            ))?;
            let mut out = Vec::new();
            let mut mapped = stmt.query_map(rusqlite::params![m.id], crate::persistence::db::row_to_media)?;
            while let Some(r) = mapped.next() {
                out.push(r?);
            }
            out
        };
        for c in children {
            let mut cdest = dest.clone();
            // sanitize child's own name too
            let seg = crate::domain::sort_template::sanitize_segment(
                &c.file_name,
                opts.template.target_os,
                opts.template.max_segment_len,
            );
            let seg = if seg.is_empty() { c.file_name.clone() } else { seg };
            cdest.push(&seg);
            let mut cdest_str = cdest.to_string_lossy().to_string();
            if used_dests.contains(&cdest_str) || Path::new(&cdest_str).exists() {
                let used = used_dests.clone();
                cdest_str = crate::domain::operation::unique_name(&cdest_str, |p| {
                    used.contains(p) || Path::new(p).exists()
                });
            }
            used_dests.insert(cdest_str.clone());
            plan_items.push(PlanItem {
                id: uuid::Uuid::new_v4().to_string(),
                plan_id: plan_id.clone(),
                media_id: c.id.clone(),
                source_path: c.path.clone(),
                destination_path: cdest_str,
                mode: opts.mode,
                status: PlanItemStatus::Pending,
                size_bytes: c.size_bytes,
                sha256_before: c.sha256.clone(),
                sha256_after: None,
                warnings: vec![PlanWarning {
                    code: "sidecar".into(),
                    message: "Переноситься разом із основним файлом".into(),
                }],
                sidecar_of: Some(parent_pi_id.clone()),
                place_override: None,
                error: None,
                finished_at: None,
                undo_source_path: None,
                undo_dest_path: None,
            });
            summary.total += 1;
            summary.expected_bytes += c.size_bytes;
        }
    }

    // persist
    conn.execute(
        "INSERT INTO operation_plan(id, created_at, mode, destination_root, template, settings_json)
         VALUES (?1,?2,?3,?4,?5,?6)",
        rusqlite::params![
            plan_id,
            crate::persistence::db::now_ms(),
            if opts.mode == OperationMode::Copy { "copy" } else { "move" },
            opts.destination_root,
            opts.template.template,
            serde_json::to_string(&opts.template).unwrap_or_default(),
        ],
    )?;
    crate::persistence::db::save_plan_items(conn, &plan_items)?;
    Ok((plan_id, plan_items, summary))
}


/// Recompute one item's destination after the user edits place/date,
/// validating containment the same way as the full plan.
pub fn recompute_item_dest(
    conn: &Connection,
    plan_id: &str,
    item_id: &str,
    new_place: Option<&str>,
    new_date_local: Option<&str>,
) -> KResult<String> {
    let meta = crate::persistence::db::get_plan_meta(conn, plan_id)?
        .ok_or_else(|| KadrError::user("План не знайдено"))?;
    let settings_json: String = conn.query_row(
        "SELECT settings_json FROM operation_plan WHERE id=?1",
        rusqlite::params![plan_id],
        |r| r.get(0),
    )?;
    let template: TemplateConfig =
        serde_json::from_str(&settings_json).unwrap_or_else(|_| TemplateConfig::default());

    let item = conn
        .query_row(
            "SELECT media_id, source_path FROM operation_item WHERE id=?1 AND plan_id=?2",
            rusqlite::params![item_id, plan_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .optional_none()?;

    let media = crate::persistence::db::get_media_item(conn, &item.0)?
        .ok_or_else(|| KadrError::user("Файл не знайдено в індексі"))?;

    let year_month = match new_date_local {
        Some(raw) => {
            let d = crate::domain::captured_date::from_manual(raw);
            d.captured_at_local.map(|dt| (dt.year(), dt.month()))
        }
        None => {
            let dt = parse_local(media.captured_at_local.as_ref());
            match (matches!(media.date_status, DateStatus::Confirmed | DateStatus::Fallback), dt) {
                (true, Some(dt)) => Some((dt.year(), dt.month())),
                _ => None,
            }
        }
    };
    let place = new_place.filter(|s| !s.trim().is_empty()).or(media.place_label.as_deref());
    let coords_label = match (media.latitude, media.longitude) {
        (Some(la), Some(lo)) => Some(format!("{la:.5}, {lo:.5}")),
        _ => None,
    };
    let segs = build_dest_segments(
        &template,
        &crate::domain::sort_template::PathInput {
            year_month,
            place,
            coords_label: coords_label.as_deref(),
            filename: &media.file_name,
        },
    );
    let mut dest = PathBuf::from(&meta.1);
    for s in &segs {
        dest.push(s);
    }
    if !is_inside_root(&meta.1, &dest) {
        return Err(KadrError::user("Нова назва місця виводила б файл за межі папки архіву"));
    }
    let dest_str = dest.to_string_lossy().to_string();
    crate::persistence::db::update_plan_item_destination(conn, item_id, &dest_str)?;
    Ok(dest_str)
}

trait OptionalNone {
    fn optional_none(self) -> KResult<(String, String)>;
}
impl OptionalNone for rusqlite::Result<(String, String)> {
    fn optional_none(self) -> KResult<(String, String)> {
        self.map_err(|_| KadrError::user("Елемент плану не знайдено"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::sort_template::{MonthFormat, TargetOs};
    use crate::persistence::db::*;
    use crate::services::scanner::{normalize_path, scan_root, ScanOptions};
    use std::fs;
    use std::sync::atomic::AtomicBool;

    fn tc() -> TemplateConfig {
        TemplateConfig {
            target_os: TargetOs::Unix,
            month_format: MonthFormat::NumberName,
            ..Default::default()
        }
    }

    fn fixture() -> (Connection, PathBuf, String) {
        let dir = std::env::temp_dir().join(format!("kadr-plan-{}", uuid::Uuid::new_v4()));
        let src = dir.join("src");
        let dest = dir.join("dest");
        fs::create_dir_all(&dest).unwrap();
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.jpg"), b"data-a").unwrap();
        fs::write(src.join("b.mov"), b"data-b").unwrap();
        let conn = open_db(":memory:").unwrap();
        let root = add_source_root(&conn, src.to_str().unwrap()).unwrap();
        let cancel = AtomicBool::new(false);
        scan_root(&conn, &root, &ScanOptions::default(), "j", &cancel, &|_| {}).unwrap();
        (conn, dest, src.to_string_lossy().to_string())
    }

    #[test]
    fn plan_layout_and_no_fs_changes() {
        let (conn, dest, _) = fixture();
        // assign manual date/place to one item
        let (items, _) = query_media(&conn, &MediaFilter::default()).unwrap();
        let jpg = items.iter().find(|i| i.extension == "jpg").unwrap();
        set_manual_date(&conn, &jpg.id, "2025-08-12 14:03:22").unwrap();
        set_manual_place(&conn, &[jpg.id.clone()], "Карпати").unwrap();

        let (pid, plan, summary) = build_plan(
            &conn,
            &PlanOptions {
                destination_root: dest.to_string_lossy().to_string(),
                template: tc(),
                mode: OperationMode::Copy,
                conflict: ConflictPolicy::Suffix,
                media_ids: None,
            },
        )
        .unwrap();
        assert_eq!(summary.total, 2);
        let jpg_item = plan.iter().find(|p| p.source_path.ends_with("a.jpg")).unwrap();
        let np = normalize_path(Path::new(&jpg_item.destination_path));
        let expect = normalize_path(Path::new(&dest.join("2025").join("08 - Серпень").join("Карпати").join("a.jpg")));
        assert_eq!(np, expect);
        let mov_item = plan.iter().find(|p| p.source_path.ends_with("b.mov")).unwrap();
        assert!(mov_item.destination_path.contains("Невідома дата"));
        assert!(mov_item.destination_path.contains("Без геолокації"));
        // plan wrote nothing to the filesystem
        assert!(!dest.join("2025").exists());
        let _ = pid;
    }

    #[test]
    fn conflict_suffix_does_not_overwrite() {
        let (conn, dest, _) = fixture();
        // pre-create an existing file at one planned destination
        fs::create_dir_all(dest.join("Невідома дата").join("Без геолокації")).unwrap();
        fs::write(dest.join("Невідома дата").join("Без геолокації").join("b.mov"), b"existing").unwrap();
        let (_pid, plan, summary) = build_plan(
            &conn,
            &PlanOptions {
                destination_root: dest.to_string_lossy().to_string(),
                template: tc(),
                mode: OperationMode::Copy,
                conflict: ConflictPolicy::Suffix,
                media_ids: None,
            },
        )
        .unwrap();
        assert_eq!(summary.conflicts, 1);
        let mov = plan.iter().find(|p| p.source_path.ends_with("b.mov")).unwrap();
        assert!(mov.destination_path.ends_with("b (1).mov"));
    }

    #[test]
    fn same_root_as_source_refused() {
        let (conn, _dest, src) = fixture();
        let res = build_plan(
            &conn,
            &PlanOptions {
                destination_root: src.clone(),
                template: tc(),
                mode: OperationMode::Copy,
                conflict: ConflictPolicy::Suffix,
                media_ids: None,
            },
        );
        assert!(res.is_err());
    }

    #[test]
    fn manual_override_recomputes_dest_safely() {
        let (conn, dest, _) = fixture();
        let (pid, plan, _) = build_plan(
            &conn,
            &PlanOptions {
                destination_root: dest.to_string_lossy().to_string(),
                template: tc(),
                mode: OperationMode::Copy,
                conflict: ConflictPolicy::Suffix,
                media_ids: None,
            },
        )
        .unwrap();
        let item = plan.iter().find(|p| p.source_path.ends_with("a.jpg")).unwrap();
        // traversal attempt must be blocked or sanitized
        let res = recompute_item_dest(&conn, &pid, &item.id, Some("../../etc/passwd"), None);
        match res {
            Ok(new_dest) => {
                assert!(is_inside_root(&dest.to_string_lossy(), Path::new(&new_dest)));
            }
            Err(_) => {}
        }
        // normal override works
        let nd = recompute_item_dest(&conn, &pid, &item.id, Some("Львів"), Some("2024-03-01 10:00:00")).unwrap();
        assert!(nd.contains("2024"));
        assert!(nd.contains("03 - Березень"));
        assert!(nd.contains("Львів"));
    }
}
