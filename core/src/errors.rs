use thiserror::Error;

/// User-presentable error. `message` is a friendly localized string,
/// `detail` holds technical info for the diagnostic log only.
#[derive(Error, Debug)]
pub enum KadrError {
    #[error("{message}")]
    User {
        message: String,
        detail: Option<String>,
    },
    #[error("io: {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("db: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("helper `{tool}` is unavailable: {reason}")]
    Helper { tool: String, reason: String },
    #[error("cancelled")]
    Cancelled,
}

impl KadrError {
    pub fn user(message: impl Into<String>) -> Self {
        KadrError::User {
            message: message.into(),
            detail: None,
        }
    }
    pub fn user_detail(message: impl Into<String>, detail: impl Into<String>) -> Self {
        KadrError::User {
            message: message.into(),
            detail: Some(detail.into()),
        }
    }
    /// Friendly message for the UI (never raw stderr/paths dumps).
    pub fn friendly(&self) -> String {
        match self {
            KadrError::User { message, .. } => message.clone(),
            KadrError::Io { path, source } => format!("Помилка доступу до «{path}»: {source}"),
            KadrError::Db(e) => format!("Помилка бази даних: {e}"),
            KadrError::Helper { tool, reason } => {
                format!("Інструмент «{tool}» недоступний: {reason}")
            }
            KadrError::Cancelled => "Операцію скасовано".into(),
        }
    }
    pub fn detail(&self) -> Option<String> {
        match self {
            KadrError::User { detail, .. } => detail.clone(),
            KadrError::Io { source, .. } => Some(source.to_string()),
            _ => None,
        }
    }
}

pub type KResult<T> = Result<T, KadrError>;

impl From<serde_json::Error> for KadrError {
    fn from(e: serde_json::Error) -> Self {
        KadrError::user_detail(
            "Не вдалося обробити відповідь інструменту метаданих",
            e.to_string(),
        )
    }
}
