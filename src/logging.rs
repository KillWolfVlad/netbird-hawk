use std::path::Path;

use tracing_appender::{
    non_blocking::WorkerGuard,
    rolling::{Builder, Rotation},
};
use tracing_subscriber::EnvFilter;

use crate::error::{HawkError, Result};

const RETAINED_LOG_FILES: usize = 7;

pub fn initialize(log_dir: &Path) -> Result<WorkerGuard> {
    let appender = Builder::new()
        .rotation(Rotation::DAILY)
        .filename_prefix("netbird-hawk")
        .filename_suffix("jsonl")
        .max_log_files(RETAINED_LOG_FILES)
        .build(log_dir)
        .map_err(|error| HawkError::Logging(error.to_string()))?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(writer)
        .with_current_span(false)
        .with_span_list(false)
        .try_init()
        .map_err(|error| HawkError::Logging(error.to_string()))?;
    Ok(guard)
}
