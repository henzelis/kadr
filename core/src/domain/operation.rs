//! Plan / operation domain model (TECHNICAL_SPEC §10, §11).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationMode {
    /// Safe default: originals stay untouched.
    Copy,
    /// Explicit user choice; source removed only after verified copy.
    Move,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConflictPolicy {
    /// Append " (1)", " (2)", ... (default, safe)
    #[default]
    Suffix,
    /// Leave the existing file alone, skip this item
    Skip,
    /// Do nothing automatically; surface to the user
    Ask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanItemStatus {
    Pending,
    Excluded,
    Done,
    Skipped,
    Failed,
    Undone,
    NeedsUser,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanWarning {
    pub code: String,
    pub message: String,
}

/// One row of the sort plan: source → destination, no filesystem changes yet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanItem {
    pub id: String,
    pub plan_id: String,
    pub media_id: String,
    pub source_path: String,
    pub destination_path: String,
    pub mode: OperationMode,
    pub status: PlanItemStatus,
    pub size_bytes: i64,
    pub sha256_before: Option<String>,
    pub sha256_after: Option<String>,
    pub warnings: Vec<PlanWarning>,
    /// Sidecar files move alongside their parent (spec §8, §6)
    pub sidecar_of: Option<String>,
    /// User date/place overrides applied to this item
    pub place_override: Option<String>,
    pub error: Option<String>,
    pub finished_at: Option<i64>,
    pub undo_source_path: Option<String>,
    pub undo_dest_path: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlanSummary {
    pub total: usize,
    pub to_copy: usize,
    pub to_move: usize,
    pub skipped: usize,
    pub conflicts: usize,
    pub unknown_date: usize,
    pub unknown_place: usize,
    pub expected_bytes: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationLogEntry {
    pub id: i64,
    pub job_id: String,
    pub level: LogLevel,
    pub action: String,
    pub message: String,
    pub created_at: i64,
}

/// Unique suffix policy helper: `name (1).ext`, `name (2).ext` ...
/// `exists` returns whether a candidate path is taken.
pub fn unique_name<F: Fn(&str) -> bool>(dest: &str, exists: F) -> String {
    if !exists(dest) {
        return dest.to_string();
    }
    let (stem, ext) = split_ext(dest);
    for i in 1..10_000 {
        let candidate = if ext.is_empty() {
            format!("{stem} ({i})")
        } else {
            format!("{stem} ({i}).{ext}")
        };
        if !exists(&candidate) {
            return candidate;
        }
    }
    // practically unreachable; deterministic fallback
    format!("{stem} (dup).{ext}")
}

pub fn split_ext(path: &str) -> (String, String) {
    match path.rfind('.') {
        Some(i) if i > 0 && !path[i..].contains('/') && !path[i..].contains('\\') => {
            (path[..i].to_string(), path[i + 1..].to_string())
        }
        _ => (path.to_string(), String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn suffix_policy() {
        let taken: HashSet<String> = ["/a/b.jpg", "/a/b (1).jpg", "/a/b (2).jpg"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            unique_name("/a/b.jpg", |p| taken.contains(p)),
            "/a/b (3).jpg"
        );
        assert_eq!(unique_name("/a/c.jpg", |p| taken.contains(p)), "/a/c.jpg");
        assert_eq!(unique_name("/a/noext", |p| taken.contains(p)), "/a/noext");
    }

    #[test]
    fn split_ext_keeps_dots_in_dirs() {
        assert_eq!(
            split_ext("/dir.d/name.jpg"),
            ("/dir.d/name".into(), "jpg".into())
        );
        assert_eq!(split_ext("README"), ("README".into(), "".into()));
    }
}
