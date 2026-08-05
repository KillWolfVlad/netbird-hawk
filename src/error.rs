use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum HawkError {
    #[error("invalid configuration: {0}")]
    Validation(String),

    #[error("could not determine per-user application directories")]
    StateDirectoryUnavailable,

    #[error("{operation} failed for {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("could not decode {record}: {source}")]
    InvalidState {
        record: &'static str,
        #[source]
        source: serde_json::Error,
    },

    #[error("{record} uses unsupported schema version {found} (expected {expected})")]
    UnsupportedVersion {
        record: &'static str,
        found: u32,
        expected: u32,
    },

    #[error("another netbird-hawk daemon already owns the per-user instance")]
    DaemonAlreadyRunning,

    #[error("the NetBird executable was not found on PATH")]
    NetbirdNotFound,

    #[error("NetBird {operation} timed out")]
    NetbirdTimeout { operation: &'static str },

    #[error("NetBird {operation} failed with exit code {exit_code:?}")]
    NetbirdCommand {
        operation: &'static str,
        exit_code: Option<i32>,
    },

    #[error("NetBird status was unusable: {0}")]
    NetbirdStatus(String),

    #[error("could not launch detached daemon: {0}")]
    Launch(String),

    #[error("timed out waiting for daemon {operation}; {guidance}")]
    AcknowledgementTimeout {
        operation: &'static str,
        guidance: String,
    },

    #[error("logging initialization failed: {0}")]
    Logging(String),
}

impl HawkError {
    pub(crate) fn io(
        operation: &'static str,
        path: impl Into<PathBuf>,
        source: std::io::Error,
    ) -> Self {
        Self::Io {
            operation,
            path: path.into(),
            source,
        }
    }
}

pub type Result<T> = std::result::Result<T, HawkError>;
