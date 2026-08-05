use std::{collections::HashSet, fmt, path::PathBuf, str::FromStr};

use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use uuid::Uuid;

use crate::error::{HawkError, Result};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GenerationId(pub Uuid);

impl GenerationId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for GenerationId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for GenerationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LocalTime(pub NaiveTime);

impl LocalTime {
    pub fn hour(self) -> u32 {
        chrono::Timelike::hour(&self.0)
    }

    pub fn minute(self) -> u32 {
        chrono::Timelike::minute(&self.0)
    }
}

impl fmt::Display for LocalTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:{:02}", self.hour(), self.minute())
    }
}

impl FromStr for LocalTime {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        let (hour, minute) = value
            .split_once(':')
            .ok_or_else(|| "time must use H:MM or HH:MM 24-hour format".to_owned())?;
        if !(1..=2).contains(&hour.len())
            || minute.len() != 2
            || !hour.bytes().all(|byte| byte.is_ascii_digit())
            || !minute.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err("time must use H:MM or HH:MM 24-hour format".to_owned());
        }
        let hour: u32 = hour
            .parse()
            .map_err(|_| "time contains an invalid hour".to_owned())?;
        let minute: u32 = minute
            .parse()
            .map_err(|_| "time contains an invalid minute".to_owned())?;
        NaiveTime::from_hms_opt(hour, minute, 0)
            .map(Self)
            .ok_or_else(|| "time must be between 00:00 and 23:59".to_owned())
    }
}

impl Serialize for LocalTime {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for LocalTime {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}

pub fn validate_profiles(profiles: &[String]) -> Result<()> {
    if profiles.is_empty() {
        return Err(HawkError::Validation(
            "at least one profile is required".to_owned(),
        ));
    }

    let mut unique = HashSet::with_capacity(profiles.len());
    for profile in profiles {
        if profile.trim().is_empty() {
            return Err(HawkError::Validation(
                "profile names must not be empty".to_owned(),
            ));
        }
        if profile.chars().any(char::is_control) {
            return Err(HawkError::Validation(
                "profile names must not contain control characters".to_owned(),
            ));
        }
        if !unique.insert(profile) {
            return Err(HawkError::Validation(format!(
                "profile {profile:?} is duplicated; profiles must be unique"
            )));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DesiredConfig {
    pub schema_version: u32,
    pub generation: GenerationId,
    pub profiles: Vec<String>,
    pub local_time: LocalTime,
    pub netbird_executable: PathBuf,
    pub activated_at: DateTime<Utc>,
}

impl DesiredConfig {
    pub fn validate(&self) -> Result<()> {
        ensure_version("desired configuration", self.schema_version)?;
        validate_profiles(&self.profiles)?;
        if !self.netbird_executable.is_absolute() {
            return Err(HawkError::Validation(
                "the saved NetBird executable path must be absolute".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesiredRunState {
    Running,
    Stopped,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ControlIntent {
    pub schema_version: u32,
    pub desired: DesiredRunState,
    pub generation: Option<GenerationId>,
    pub requested_at: DateTime<Utc>,
}

impl ControlIntent {
    pub fn validate(&self) -> Result<()> {
        ensure_version("control intent", self.schema_version)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    Starting,
    Running,
    Degraded,
    Stopped,
}

impl fmt::Display for LifecycleState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Degraded => "degraded",
            Self::Stopped => "stopped",
        };
        f.write_str(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    ExecutableMissing,
    CommandFailed,
    TimedOut,
    MalformedStatus,
    StateIo,
    Configuration,
    Internal,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SanitizedError {
    pub category: ErrorCategory,
    pub message: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}

impl SanitizedError {
    pub fn command_failed(operation: &str, exit_code: Option<i32>) -> Self {
        Self {
            category: ErrorCategory::CommandFailed,
            message: format!("NetBird {operation} failed"),
            exit_code,
            timed_out: false,
        }
    }

    pub fn timed_out(operation: &str) -> Self {
        Self {
            category: ErrorCategory::TimedOut,
            message: format!("NetBird {operation} timed out"),
            exit_code: None,
            timed_out: true,
        }
    }

    pub fn malformed_status(reason: impl Into<String>) -> Self {
        Self {
            category: ErrorCategory::MalformedStatus,
            message: reason.into(),
            exit_code: None,
            timed_out: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalOutcome {
    PendingDiscovery,
    InProgress,
    Success,
    Superseded,
    Failed,
}

impl JournalOutcome {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Success | Self::Superseded | Self::Failed)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecutionJournal {
    pub schema_version: u32,
    pub generation: GenerationId,
    pub local_date: NaiveDate,
    pub original_profile: Option<String>,
    pub intended_target: Option<String>,
    pub attempts: u32,
    pub outcome: JournalOutcome,
    pub next_retry_at: Option<DateTime<Utc>>,
    pub last_error: Option<SanitizedError>,
    pub updated_at: DateTime<Utc>,
}

impl ExecutionJournal {
    pub fn validate(&self) -> Result<()> {
        ensure_version("execution journal", self.schema_version)?;
        if self.outcome == JournalOutcome::InProgress
            && (self.original_profile.is_none() || self.intended_target.is_none())
        {
            return Err(HawkError::Validation(
                "an in-progress journal requires original and target profiles".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeSnapshot {
    pub schema_version: u32,
    pub state: LifecycleState,
    pub applied_generation: Option<GenerationId>,
    pub pid: Option<u32>,
    pub profiles: Vec<String>,
    pub local_time: Option<LocalTime>,
    pub active_profile: Option<String>,
    pub next_profile: Option<String>,
    pub next_local_occurrence: Option<String>,
    pub last_result: Option<String>,
    pub last_error: Option<SanitizedError>,
    pub updated_at: DateTime<Utc>,
}

impl RuntimeSnapshot {
    pub fn validate(&self) -> Result<()> {
        ensure_version("runtime snapshot", self.schema_version)
    }

    pub fn stopped() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            state: LifecycleState::Stopped,
            applied_generation: None,
            pid: None,
            profiles: Vec::new(),
            local_time: None,
            active_profile: None,
            next_profile: None,
            next_local_occurrence: None,
            last_result: None,
            last_error: None,
            updated_at: Utc::now(),
        }
    }
}

pub fn ensure_version(record: &'static str, found: u32) -> Result<()> {
    if found == SCHEMA_VERSION {
        Ok(())
    } else {
        Err(HawkError::UnsupportedVersion {
            record,
            found,
            expected: SCHEMA_VERSION,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_normalizes_local_time() {
        let time: LocalTime = "8:24".parse().unwrap();
        assert_eq!(time.to_string(), "08:24");
        assert_eq!(serde_json::to_string(&time).unwrap(), "\"08:24\"");
    }

    #[test]
    fn rejects_invalid_local_times() {
        for value in ["", "8", "8:2", "008:02", "24:00", "12:60", "-1:00"] {
            assert!(value.parse::<LocalTime>().is_err(), "accepted {value}");
        }
    }

    #[test]
    fn preserves_order_and_rejects_duplicates_or_empty_profiles() {
        assert!(validate_profiles(&["alpha".into(), "beta".into()]).is_ok());
        assert!(validate_profiles(&["alpha".into(), "alpha".into()]).is_err());
        assert!(validate_profiles(&[" ".into()]).is_err());
    }
}
