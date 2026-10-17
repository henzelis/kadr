//! Kadr core: domain model, scanning, metadata, planning and safe operations.
//! Deliberately independent of Tauri and any UI (TECHNICAL_SPEC §15.8).

pub mod domain {
    pub mod captured_date;
    pub mod location;
    pub mod media_item;
    pub mod operation;
    pub mod sort_template;
}

pub mod errors;
pub mod persistence {
    pub mod db;
}
pub mod services {
    pub mod duplicate;
    pub mod geocode;
    pub mod metadata;
    pub mod operation_runner;
    pub mod path_planner;
    pub mod report;
    pub mod scanner;
    pub mod thumbnail;
}

pub use errors::{KResult, KadrError};
