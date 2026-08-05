use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use chrono::Utc;

use crate::{
    error::{HawkError, Result},
    model::{
        ControlIntent, DesiredConfig, DesiredRunState, GenerationId, LifecycleState, LocalTime,
        RuntimeSnapshot, SCHEMA_VERSION,
    },
    netbird::{NetbirdApi, operational_error},
    platform::ProcessLauncher,
    state::StateStore,
};

pub const DEFAULT_ACKNOWLEDGEMENT_TIMEOUT: Duration = Duration::from_secs(12);
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug)]
pub struct LifecycleController<L> {
    store: StateStore,
    launcher: L,
    acknowledgement_timeout: Duration,
    poll_interval: Duration,
}

impl<L: ProcessLauncher> LifecycleController<L> {
    pub fn new(store: StateStore, launcher: L) -> Self {
        Self {
            store,
            launcher,
            acknowledgement_timeout: DEFAULT_ACKNOWLEDGEMENT_TIMEOUT,
            poll_interval: DEFAULT_POLL_INTERVAL,
        }
    }

    #[cfg(test)]
    pub fn with_timeouts(mut self, acknowledgement: Duration, poll: Duration) -> Self {
        self.acknowledgement_timeout = acknowledgement;
        self.poll_interval = poll;
        self
    }

    pub fn store(&self) -> &StateStore {
        &self.store
    }

    pub async fn start<A: NetbirdApi>(
        &self,
        api: &A,
        netbird_executable: PathBuf,
        current_executable: &Path,
        profiles: Vec<String>,
        local_time: LocalTime,
    ) -> Result<String> {
        crate::model::validate_profiles(&profiles)?;
        if !netbird_executable.is_absolute() {
            return Err(HawkError::Validation(
                "NetBird executable discovery did not return an absolute path".to_owned(),
            ));
        }
        self.store.ensure_layout()?;
        let _controller_lock = self.store.controller_lock()?;

        // Preflight happens before replacing a known-good configuration.
        let preflight_profile = api
            .active_profile()
            .await
            .map_err(|error| operational_error(&error))?;
        let was_live = self.store.daemon_is_live()?;
        let previous_generation = self.store.read_config()?.map(|config| config.generation);
        let generation = GenerationId::new();
        let config = DesiredConfig {
            schema_version: SCHEMA_VERSION,
            generation,
            profiles: profiles.clone(),
            local_time,
            netbird_executable,
            activated_at: Utc::now(),
        };
        self.store.write_config(&config)?;
        self.store.write_control(&ControlIntent {
            schema_version: SCHEMA_VERSION,
            desired: DesiredRunState::Running,
            generation: Some(generation),
            requested_at: Utc::now(),
        })?;

        if !was_live {
            self.store.write_runtime(&RuntimeSnapshot {
                schema_version: SCHEMA_VERSION,
                state: LifecycleState::Starting,
                applied_generation: None,
                pid: None,
                profiles: profiles.clone(),
                local_time: Some(local_time),
                active_profile: Some(preflight_profile),
                next_profile: None,
                next_local_occurrence: None,
                last_result: None,
                last_error: None,
                updated_at: Utc::now(),
            })?;
            self.launcher.launch_worker(current_executable)?;
        }

        self.wait_for_generation(generation, previous_generation)
            .await?;
        Ok(format!(
            "daemon {} generation {generation}\nprofiles: {}\ntime: {local_time} local",
            if was_live {
                "reconfigured to"
            } else {
                "started with"
            },
            profiles.join(" -> ")
        ))
    }

    pub async fn stop(&self) -> Result<String> {
        self.store.ensure_layout()?;
        let _controller_lock = self.store.controller_lock()?;
        if !self.store.daemon_is_live()? {
            return Ok("daemon is already stopped".to_owned());
        }

        let generation = self.store.read_config()?.map(|config| config.generation);
        self.store.write_control(&ControlIntent {
            schema_version: SCHEMA_VERSION,
            desired: DesiredRunState::Stopped,
            generation,
            requested_at: Utc::now(),
        })?;

        let deadline = tokio::time::Instant::now() + self.acknowledgement_timeout;
        loop {
            if !self.store.daemon_is_live()? {
                return Ok("daemon stopped".to_owned());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(HawkError::AcknowledgementTimeout {
                    operation: "shutdown acknowledgement",
                    guidance: "the process was not killed from PID metadata; inspect the log and retry stop"
                        .to_owned(),
                });
            }
            tokio::time::sleep(self.poll_interval).await;
        }
    }

    pub fn status(&self) -> Result<String> {
        self.store.ensure_layout()?;
        if !self.store.daemon_is_live()? {
            return Ok("state: stopped".to_owned());
        }

        let snapshot = match self.store.read_runtime() {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) => {
                return Ok("state: degraded\ndetails: runtime snapshot is unavailable".to_owned());
            }
            Err(_) => {
                return Ok("state: degraded\ndetails: runtime snapshot is unreadable".to_owned());
            }
        };
        Ok(format_snapshot(&snapshot))
    }

    async fn wait_for_generation(
        &self,
        generation: GenerationId,
        previous_generation: Option<GenerationId>,
    ) -> Result<()> {
        let deadline = tokio::time::Instant::now() + self.acknowledgement_timeout;
        loop {
            if let Ok(Some(snapshot)) = self.store.read_runtime()
                && snapshot.applied_generation == Some(generation)
                && snapshot.state == LifecycleState::Running
            {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                let observed = self
                    .store
                    .read_runtime()
                    .ok()
                    .flatten()
                    .and_then(|snapshot| snapshot.applied_generation);
                let guidance = match observed {
                    Some(observed) if Some(observed) == previous_generation => {
                        format!("the daemon still reports its previous generation {observed}")
                    }
                    Some(observed) => format!("the daemon reports generation {observed}"),
                    None => "no ready generation was published; inspect the per-user log and retry"
                        .to_owned(),
                };
                return Err(HawkError::AcknowledgementTimeout {
                    operation: "ready acknowledgement",
                    guidance,
                });
            }
            tokio::time::sleep(self.poll_interval).await;
        }
    }
}

fn format_snapshot(snapshot: &RuntimeSnapshot) -> String {
    let mut lines = vec![format!("state: {}", snapshot.state)];
    if !snapshot.profiles.is_empty() {
        lines.push(format!("profiles: {}", snapshot.profiles.join(" -> ")));
    }
    if let Some(time) = snapshot.local_time {
        lines.push(format!("time: {time} local"));
    }
    if let Some(profile) = &snapshot.active_profile {
        lines.push(format!("active profile: {profile}"));
    }
    if let Some(profile) = &snapshot.next_profile {
        lines.push(format!("next profile: {profile}"));
    }
    if let Some(occurrence) = &snapshot.next_local_occurrence {
        lines.push(format!("next occurrence: {occurrence}"));
    }
    if let Some(result) = &snapshot.last_result {
        lines.push(format!("last result: {result}"));
    }
    if let Some(error) = &snapshot.last_error {
        lines.push(format!("last error: {}", error.message));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use tempfile::tempdir;

    use crate::{
        model::{ErrorCategory, SanitizedError},
        platform::ProcessLauncher,
    };

    use super::*;

    #[derive(Debug)]
    struct FakeApi {
        result: std::result::Result<String, SanitizedError>,
    }

    #[async_trait]
    impl NetbirdApi for FakeApi {
        async fn active_profile(&self) -> std::result::Result<String, SanitizedError> {
            self.result.clone()
        }

        async fn select_profile(&self, _target: &str) -> std::result::Result<(), SanitizedError> {
            Ok(())
        }
    }

    #[derive(Clone, Debug)]
    struct AcknowledgingLauncher {
        store: StateStore,
        launches: Arc<Mutex<u32>>,
    }

    impl ProcessLauncher for AcknowledgingLauncher {
        fn launch_worker(&self, _current_executable: &Path) -> Result<u32> {
            *self.launches.lock().unwrap() += 1;
            let config = self.store.read_config()?.unwrap();
            self.store.write_runtime(&RuntimeSnapshot {
                schema_version: SCHEMA_VERSION,
                state: LifecycleState::Running,
                applied_generation: Some(config.generation),
                pid: Some(7),
                profiles: config.profiles,
                local_time: Some(config.local_time),
                active_profile: Some("alpha".into()),
                next_profile: Some("beta".into()),
                next_local_occurrence: None,
                last_result: None,
                last_error: None,
                updated_at: Utc::now(),
            })?;
            Ok(7)
        }
    }

    #[derive(Clone, Copy, Debug)]
    struct NoopLauncher;

    impl ProcessLauncher for NoopLauncher {
        fn launch_worker(&self, _current_executable: &Path) -> Result<u32> {
            Ok(7)
        }
    }

    #[tokio::test]
    async fn initial_start_waits_for_ready_acknowledgement() {
        let root = tempdir().unwrap();
        let store = StateStore::under(root.path());
        store.ensure_layout().unwrap();
        let launches = Arc::new(Mutex::new(0));
        let controller = LifecycleController::new(
            store.clone(),
            AcknowledgingLauncher {
                store,
                launches: launches.clone(),
            },
        );
        let output = controller
            .start(
                &FakeApi {
                    result: Ok("alpha".into()),
                },
                std::env::current_exe().unwrap(),
                &std::env::current_exe().unwrap(),
                vec!["alpha".into(), "beta".into()],
                "8:24".parse().unwrap(),
            )
            .await
            .unwrap();
        assert!(output.contains("started"));
        assert_eq!(*launches.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn invalid_preflight_preserves_existing_configuration() {
        let root = tempdir().unwrap();
        let store = StateStore::under(root.path());
        store.ensure_layout().unwrap();
        let original = DesiredConfig {
            schema_version: SCHEMA_VERSION,
            generation: GenerationId::new(),
            profiles: vec!["old".into()],
            local_time: "08:00".parse().unwrap(),
            netbird_executable: std::env::current_exe().unwrap(),
            activated_at: Utc::now(),
        };
        store.write_config(&original).unwrap();
        let controller = LifecycleController::new(store.clone(), NoopLauncher);
        let error = SanitizedError {
            category: ErrorCategory::MalformedStatus,
            message: "the Profile field was missing".into(),
            exit_code: None,
            timed_out: false,
        };
        assert!(
            controller
                .start(
                    &FakeApi { result: Err(error) },
                    std::env::current_exe().unwrap(),
                    &std::env::current_exe().unwrap(),
                    vec!["new".into()],
                    "09:00".parse().unwrap(),
                )
                .await
                .is_err()
        );
        assert_eq!(store.read_config().unwrap(), Some(original));
    }

    #[test]
    fn stopped_status_ignores_stale_pid_metadata() {
        let root = tempdir().unwrap();
        let store = StateStore::under(root.path());
        store.ensure_layout().unwrap();
        let mut stale = RuntimeSnapshot::stopped();
        stale.state = LifecycleState::Running;
        stale.pid = Some(1234);
        store.write_runtime(&stale).unwrap();
        let controller = LifecycleController::new(store, NoopLauncher);
        assert_eq!(controller.status().unwrap(), "state: stopped");
    }

    #[test]
    fn live_but_unreadable_runtime_is_reported_as_degraded() {
        let root = tempdir().unwrap();
        let store = StateStore::under(root.path());
        store.ensure_layout().unwrap();
        std::fs::write(store.runtime_path(), b"not json").unwrap();
        let _lifetime = store.try_lifetime_lock().unwrap().unwrap();
        let controller = LifecycleController::new(store, NoopLauncher);
        assert!(controller.status().unwrap().contains("state: degraded"));
    }

    #[tokio::test]
    async fn stop_timeout_does_not_break_lifetime_ownership() {
        let root = tempdir().unwrap();
        let store = StateStore::under(root.path());
        store.ensure_layout().unwrap();
        let lifetime = store.try_lifetime_lock().unwrap().unwrap();
        let controller = LifecycleController::new(store.clone(), NoopLauncher)
            .with_timeouts(Duration::from_millis(20), Duration::from_millis(2));
        assert!(matches!(
            controller.stop().await,
            Err(HawkError::AcknowledgementTimeout { .. })
        ));
        assert!(store.daemon_is_live().unwrap());
        drop(lifetime);
    }

    #[tokio::test]
    async fn owned_lifetime_lock_drives_live_status_and_bounded_stop() {
        let root = tempdir().unwrap();
        let store = StateStore::under(root.path());
        store.ensure_layout().unwrap();
        let mut snapshot = RuntimeSnapshot::stopped();
        snapshot.state = LifecycleState::Running;
        snapshot.pid = Some(7);
        store.write_runtime(&snapshot).unwrap();
        let lifetime = store.try_lifetime_lock().unwrap().unwrap();
        let controller = LifecycleController::new(store.clone(), NoopLauncher)
            .with_timeouts(Duration::from_millis(20), Duration::from_millis(2));

        assert!(store.daemon_is_live().unwrap());
        assert!(controller.status().unwrap().starts_with("state: running"));
        let started = std::time::Instant::now();
        assert!(matches!(
            controller.stop().await,
            Err(HawkError::AcknowledgementTimeout { .. })
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(
            store.read_control().unwrap().unwrap().desired,
            DesiredRunState::Stopped
        );
        assert!(store.daemon_is_live().unwrap());

        drop(lifetime);
        assert_eq!(controller.status().unwrap(), "state: stopped");
    }

    #[tokio::test]
    async fn ready_acknowledgement_timeout_reports_failure() {
        let root = tempdir().unwrap();
        let store = StateStore::under(root.path());
        store.ensure_layout().unwrap();
        let controller = LifecycleController::new(store, NoopLauncher)
            .with_timeouts(Duration::from_millis(20), Duration::from_millis(2));
        let result = controller
            .start(
                &FakeApi {
                    result: Ok("alpha".into()),
                },
                std::env::current_exe().unwrap(),
                &std::env::current_exe().unwrap(),
                vec!["alpha".into()],
                "08:00".parse().unwrap(),
            )
            .await;
        assert!(matches!(
            result,
            Err(HawkError::AcknowledgementTimeout { .. })
        ));
    }
}
