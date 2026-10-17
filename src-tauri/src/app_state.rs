use kadr_core::services::thumbnail::ThumbService;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub struct AppState {
    pub db_path: PathBuf,
    pub scan_cancel: Arc<AtomicBool>,
    pub op_cancel: Arc<AtomicBool>,
    pub scan_running: Arc<AtomicBool>,
    pub op_running: Arc<AtomicBool>,
    pub thumbs: Arc<ThumbService>,
}

impl AppState {
    pub fn new(data_dir: &std::path::Path) -> Self {
        Self {
            db_path: data_dir.join("kadr.db"),
            scan_cancel: Arc::new(AtomicBool::new(false)),
            op_cancel: Arc::new(AtomicBool::new(false)),
            scan_running: Arc::new(AtomicBool::new(false)),
            op_running: Arc::new(AtomicBool::new(false)),
            thumbs: Arc::new(ThumbService::new(&data_dir.join("thumbs"))),
        }
    }

    pub fn conn(&self) -> kadr_core::KResult<rusqlite::Connection> {
        kadr_core::persistence::db::open_db(&self.db_path.to_string_lossy())
    }
}

/// Serialize a core error for the frontend: friendly message only.
pub fn err_str(e: kadr_core::KadrError) -> String {
    e.friendly()
}
