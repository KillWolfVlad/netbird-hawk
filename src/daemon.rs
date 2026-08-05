use std::time::Duration;

use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};

use crate::{
    error::{HawkError, Result},
    logging,
    model::{
        DesiredConfig, DesiredRunState, ExecutionJournal, JournalOutcome, LifecycleState,
        RuntimeSnapshot, SCHEMA_VERSION, SanitizedError,
    },
    netbird::{NetbirdApi, NetbirdClient, TokioCommandRunner},
    scheduler::{WALL_CLOCK_GUARD_SECONDS, circular_successor, latest_due_date, next_occurrence},
    state::StateStore,
};

pub const MAX_ATTEMPTS: u32 = 3;
const RETRY_DELAYS_SECONDS: [i64; MAX_ATTEMPTS as usize] = [5, 30, 120];

pub trait Clock: Send + Sync {
    type Zone: TimeZone + Clone + Send + Sync;

    fn timezone(&self) -> Self::Zone;
    fn now(&self) -> DateTime<Self::Zone>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LocalClock;

impl Clock for LocalClock {
    type Zone = Local;

    fn timezone(&self) -> Self::Zone {
        Local
    }

    fn now(&self) -> DateTime<Self::Zone> {
        Local::now()
    }
}

#[derive(Debug)]
pub struct RotationStep {
    pub active_profile: Option<String>,
    pub outcome: JournalOutcome,
    pub error: Option<SanitizedError>,
}

pub async fn run_worker(store: StateStore) -> Result<()> {
    store.ensure_layout()?;
    let _log_guard = logging::initialize(&store.paths().log_dir)?;
    let _lifetime_lock = store
        .try_lifetime_lock()?
        .ok_or(HawkError::DaemonAlreadyRunning)?;
    tracing::info!(event = "daemon_start", "daemon acquired lifetime ownership");

    let config = store.read_config()?.ok_or_else(|| {
        HawkError::Validation("the daemon has no desired configuration".to_owned())
    })?;
    let control = store
        .read_control()?
        .ok_or_else(|| HawkError::Validation("the daemon has no control intent".to_owned()))?;
    if control.desired != DesiredRunState::Running || control.generation != Some(config.generation)
    {
        publish_stopped(&store, Some(&config))?;
        return Err(HawkError::Validation(
            "configuration and running control intent do not name the same generation".to_owned(),
        ));
    }

    let client = NetbirdClient::new(
        config.netbird_executable.clone(),
        TokioCommandRunner,
        crate::netbird::DEFAULT_COMMAND_TIMEOUT,
    )?;
    run_loop(store, config, client, LocalClock).await
}

pub async fn run_loop<A, C>(
    store: StateStore,
    mut config: DesiredConfig,
    mut api: A,
    clock: C,
) -> Result<()>
where
    A: NetbirdApi,
    C: Clock,
    C::Zone: TimeZone,
    <C::Zone as TimeZone>::Offset: Copy,
{
    let mut active_profile = None;
    let mut startup_error = None;
    match api.active_profile().await {
        Ok(profile) => active_profile = Some(profile),
        Err(error) => startup_error = Some(error),
    }
    publish_runtime(
        &store,
        &config,
        RuntimePublication {
            state: if startup_error.is_some() {
                LifecycleState::Degraded
            } else {
                LifecycleState::Running
            },
            applied_generation: startup_error.is_none().then_some(config.generation),
            active_profile: active_profile.clone(),
            next_local_occurrence: None,
            last_result: None,
            last_error: startup_error,
        },
    )?;

    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(WALL_CLOCK_GUARD_SECONDS)) => {},
            signal = tokio::signal::ctrl_c() => {
                if signal.is_ok() {
                    tracing::info!(event = "daemon_signal", "daemon received a termination signal");
                    publish_stopped(&store, Some(&config))?;
                    return Ok(());
                }
            }
        }

        let control = store.read_control()?;
        if control
            .as_ref()
            .is_some_and(|intent| intent.desired == DesiredRunState::Stopped)
        {
            tracing::info!(event = "daemon_stop", "daemon acknowledged stop intent");
            publish_stopped(&store, Some(&config))?;
            return Ok(());
        }

        let persisted_desired = store.read_config()?.ok_or_else(|| {
            HawkError::Validation("desired configuration disappeared while running".to_owned())
        })?;
        // Config and control are separate atomic records. Only their matching
        // generation authorizes a swap, so a controller crash between writes
        // cannot partially apply a replacement.
        let desired = if control
            .as_ref()
            .is_some_and(|intent| intent.generation == Some(persisted_desired.generation))
        {
            persisted_desired
        } else {
            config.clone()
        };
        let generation_changed = desired.generation != config.generation;
        if generation_changed || active_profile.is_none() {
            let previous_executable = config.netbird_executable.clone();
            if generation_changed {
                api.set_executable(&desired.netbird_executable)
                    .map_err(|error| HawkError::Validation(error.message))?;
            }
            match api.active_profile().await {
                Ok(profile) => {
                    if generation_changed {
                        tracing::info!(
                            event = "configuration_applied",
                            generation = %desired.generation,
                            "daemon applied a complete configuration generation"
                        );
                        config = desired;
                    }
                    active_profile = Some(profile);
                }
                Err(error) => {
                    if generation_changed {
                        let _ = api.set_executable(&previous_executable);
                    }
                    publish_runtime(
                        &store,
                        &config,
                        RuntimePublication {
                            state: LifecycleState::Degraded,
                            applied_generation: active_profile.as_ref().map(|_| config.generation),
                            active_profile: active_profile.clone(),
                            next_local_occurrence: None,
                            last_result: None,
                            last_error: Some(error),
                        },
                    )?;
                    continue;
                }
            }
        }

        let timezone = clock.timezone();
        let now = clock.now();
        let now_utc = now.with_timezone(&Utc);
        let activated = config.activated_at.with_timezone(&timezone);
        let journal = store.read_journal()?;
        let handled = journal
            .as_ref()
            .filter(|entry| entry.generation == config.generation && entry.outcome.is_terminal())
            .map(|entry| entry.local_date);
        let due = latest_due_date(&timezone, now, activated, config.local_time, handled);

        let mut last_error = None;
        if let Some(date) = due {
            let step = run_occurrence_step(&store, &config, &api, date, now_utc).await?;
            if let Some(profile) = step.active_profile {
                active_profile = Some(profile);
            }
            last_error = step.error;
        }

        let journal = store.read_journal()?;
        if last_error.is_none() {
            last_error = journal.as_ref().and_then(|entry| entry.last_error.clone());
        }
        let handled = journal
            .as_ref()
            .filter(|entry| entry.generation == config.generation && entry.outcome.is_terminal())
            .map(|entry| entry.local_date);
        let next = next_occurrence(&timezone, now, activated, config.local_time, handled);
        let last_result = journal
            .as_ref()
            .filter(|entry| entry.generation == config.generation)
            .map(|entry| format!("{}: {:?}", entry.local_date, entry.outcome).to_lowercase());
        publish_runtime(
            &store,
            &config,
            RuntimePublication {
                state: if last_error.is_some() {
                    LifecycleState::Degraded
                } else {
                    LifecycleState::Running
                },
                applied_generation: Some(config.generation),
                active_profile: active_profile.clone(),
                next_local_occurrence: Some(next.to_rfc3339()),
                last_result,
                last_error,
            },
        )?;
    }
}

pub async fn run_occurrence_step<A: NetbirdApi>(
    store: &StateStore,
    config: &DesiredConfig,
    api: &A,
    local_date: NaiveDate,
    now: DateTime<Utc>,
) -> Result<RotationStep> {
    let existing = store
        .read_journal()?
        .filter(|entry| entry.generation == config.generation && entry.local_date == local_date);
    if let Some(entry) = &existing {
        if entry.outcome.is_terminal() {
            return Ok(RotationStep {
                active_profile: (entry.outcome == JournalOutcome::Success)
                    .then(|| entry.intended_target.clone())
                    .flatten(),
                outcome: entry.outcome,
                error: entry.last_error.clone(),
            });
        }
        if entry.next_retry_at.is_some_and(|retry| retry > now) {
            return Ok(RotationStep {
                active_profile: None,
                outcome: entry.outcome,
                error: entry.last_error.clone(),
            });
        }
    }

    let current = match api.active_profile().await {
        Ok(profile) => profile,
        Err(error) => {
            let attempts = existing.as_ref().map_or(1, |entry| entry.attempts + 1);
            let outcome = if attempts >= MAX_ATTEMPTS {
                JournalOutcome::Failed
            } else {
                JournalOutcome::PendingDiscovery
            };
            let journal = ExecutionJournal {
                schema_version: SCHEMA_VERSION,
                generation: config.generation,
                local_date,
                original_profile: None,
                intended_target: None,
                attempts,
                outcome,
                next_retry_at: retry_at(now, attempts, outcome),
                last_error: Some(error.clone()),
                updated_at: now,
            };
            store.write_journal(&journal)?;
            log_sanitized_failure(&error);
            return Ok(RotationStep {
                active_profile: None,
                outcome,
                error: Some(error),
            });
        }
    };

    if let Some(entry) = &existing
        && entry.outcome == JournalOutcome::InProgress
    {
        let original = entry.original_profile.as_deref().unwrap_or_default();
        let target = entry.intended_target.as_deref().unwrap_or_default();
        if current == target {
            return complete_journal(store, entry.clone(), JournalOutcome::Success, current, now);
        }
        if current != original {
            return complete_journal(
                store,
                entry.clone(),
                JournalOutcome::Superseded,
                current,
                now,
            );
        }
    }

    let (target, mut journal) = if let Some(entry) = existing
        && entry.outcome == JournalOutcome::InProgress
    {
        (entry.intended_target.clone().unwrap_or_default(), entry)
    } else {
        let target = circular_successor(&config.profiles, &current)
            .ok_or_else(|| HawkError::Validation("profile list became empty".to_owned()))?
            .to_owned();
        let entry = ExecutionJournal {
            schema_version: SCHEMA_VERSION,
            generation: config.generation,
            local_date,
            original_profile: Some(current.clone()),
            intended_target: Some(target.clone()),
            attempts: 0,
            outcome: JournalOutcome::InProgress,
            next_retry_at: None,
            last_error: None,
            updated_at: now,
        };
        // Write-ahead intent is durable before the external side effect.
        store.write_journal(&entry)?;
        (target, entry)
    };

    match api.select_profile(&target).await {
        Ok(()) => complete_journal(store, journal, JournalOutcome::Success, target, now),
        Err(error) => {
            journal.attempts += 1;
            journal.outcome = if journal.attempts >= MAX_ATTEMPTS {
                JournalOutcome::Failed
            } else {
                JournalOutcome::InProgress
            };
            journal.next_retry_at = retry_at(now, journal.attempts, journal.outcome);
            journal.last_error = Some(error.clone());
            journal.updated_at = now;
            store.write_journal(&journal)?;
            log_sanitized_failure(&error);
            Ok(RotationStep {
                active_profile: Some(current),
                outcome: journal.outcome,
                error: Some(error),
            })
        }
    }
}

fn retry_at(now: DateTime<Utc>, attempts: u32, outcome: JournalOutcome) -> Option<DateTime<Utc>> {
    if outcome.is_terminal() {
        return None;
    }
    let index = attempts.saturating_sub(1) as usize;
    let bounded_index = index.min(RETRY_DELAYS_SECONDS.len() - 1);
    Some(now + chrono::Duration::seconds(RETRY_DELAYS_SECONDS[bounded_index]))
}

fn complete_journal(
    store: &StateStore,
    mut journal: ExecutionJournal,
    outcome: JournalOutcome,
    active_profile: String,
    now: DateTime<Utc>,
) -> Result<RotationStep> {
    journal.outcome = outcome;
    journal.next_retry_at = None;
    journal.last_error = None;
    journal.updated_at = now;
    store.write_journal(&journal)?;
    tracing::info!(event = "rotation_complete", outcome = ?outcome, "rotation occurrence completed");
    Ok(RotationStep {
        active_profile: Some(active_profile),
        outcome,
        error: None,
    })
}

struct RuntimePublication {
    state: LifecycleState,
    applied_generation: Option<crate::model::GenerationId>,
    active_profile: Option<String>,
    next_local_occurrence: Option<String>,
    last_result: Option<String>,
    last_error: Option<SanitizedError>,
}

fn publish_runtime(
    store: &StateStore,
    config: &DesiredConfig,
    publication: RuntimePublication,
) -> Result<()> {
    let next_profile = publication
        .active_profile
        .as_deref()
        .and_then(|active| circular_successor(&config.profiles, active))
        .map(str::to_owned);
    store.write_runtime(&RuntimeSnapshot {
        schema_version: SCHEMA_VERSION,
        state: publication.state,
        applied_generation: publication.applied_generation,
        pid: Some(std::process::id()),
        profiles: config.profiles.clone(),
        local_time: Some(config.local_time),
        active_profile: publication.active_profile,
        next_profile,
        next_local_occurrence: publication.next_local_occurrence,
        last_result: publication.last_result,
        last_error: publication.last_error,
        updated_at: Utc::now(),
    })
}

fn publish_stopped(store: &StateStore, config: Option<&DesiredConfig>) -> Result<()> {
    let mut snapshot = RuntimeSnapshot::stopped();
    if let Some(config) = config {
        snapshot.applied_generation = Some(config.generation);
        snapshot.profiles = config.profiles.clone();
        snapshot.local_time = Some(config.local_time);
    }
    store.write_runtime(&snapshot)
}

fn log_sanitized_failure(error: &SanitizedError) {
    tracing::warn!(
        event = "netbird_failure",
        category = ?error.category,
        exit_code = error.exit_code,
        timed_out = error.timed_out,
        "NetBird operation failed"
    );
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, sync::Mutex};

    use async_trait::async_trait;
    use tempfile::tempdir;

    use crate::model::{ErrorCategory, GenerationId, LocalTime};

    use super::*;

    #[derive(Debug)]
    struct FakeNetbird {
        statuses: Mutex<VecDeque<std::result::Result<String, SanitizedError>>>,
        selections: Mutex<Vec<String>>,
        selection_results: Mutex<VecDeque<std::result::Result<(), SanitizedError>>>,
    }

    impl FakeNetbird {
        fn new(
            statuses: &[&str],
            selection_results: Vec<std::result::Result<(), SanitizedError>>,
        ) -> Self {
            Self {
                statuses: Mutex::new(
                    statuses
                        .iter()
                        .map(|value| Ok((*value).to_owned()))
                        .collect(),
                ),
                selections: Mutex::new(Vec::new()),
                selection_results: Mutex::new(selection_results.into()),
            }
        }
    }

    #[async_trait]
    impl NetbirdApi for FakeNetbird {
        async fn active_profile(&self) -> std::result::Result<String, SanitizedError> {
            self.statuses.lock().unwrap().pop_front().unwrap()
        }

        async fn select_profile(&self, target: &str) -> std::result::Result<(), SanitizedError> {
            self.selections.lock().unwrap().push(target.to_owned());
            self.selection_results
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(()))
        }
    }

    fn config() -> DesiredConfig {
        DesiredConfig {
            schema_version: SCHEMA_VERSION,
            generation: GenerationId::new(),
            profiles: vec!["alpha".into(), "beta".into(), "gamma".into()],
            local_time: "08:00".parse::<LocalTime>().unwrap(),
            netbird_executable: std::env::current_exe().unwrap(),
            activated_at: Utc::now() - chrono::Duration::days(2),
        }
    }

    fn failure() -> SanitizedError {
        SanitizedError {
            category: ErrorCategory::CommandFailed,
            message: "NetBird profile selection failed".into(),
            exit_code: Some(1),
            timed_out: false,
        }
    }

    #[tokio::test]
    async fn daily_selection_uses_manual_active_profile() {
        let root = tempdir().unwrap();
        let store = StateStore::under(root.path());
        store.ensure_layout().unwrap();
        let config = config();
        let api = FakeNetbird::new(&["beta"], vec![Ok(())]);
        let date = Utc::now().date_naive();
        let step = run_occurrence_step(&store, &config, &api, date, Utc::now())
            .await
            .unwrap();
        assert_eq!(step.outcome, JournalOutcome::Success);
        assert_eq!(&*api.selections.lock().unwrap(), &["gamma"]);
    }

    #[tokio::test]
    async fn crash_window_reconciles_target_without_duplicate_selection() {
        let root = tempdir().unwrap();
        let store = StateStore::under(root.path());
        store.ensure_layout().unwrap();
        let config = config();
        let date = Utc::now().date_naive();
        store
            .write_journal(&ExecutionJournal {
                schema_version: SCHEMA_VERSION,
                generation: config.generation,
                local_date: date,
                original_profile: Some("alpha".into()),
                intended_target: Some("beta".into()),
                attempts: 1,
                outcome: JournalOutcome::InProgress,
                next_retry_at: None,
                last_error: Some(failure()),
                updated_at: Utc::now(),
            })
            .unwrap();
        let api = FakeNetbird::new(&["beta"], vec![]);
        let step = run_occurrence_step(&store, &config, &api, date, Utc::now())
            .await
            .unwrap();
        assert_eq!(step.outcome, JournalOutcome::Success);
        assert!(api.selections.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn retry_uses_same_target_and_manual_third_profile_supersedes_it() {
        let root = tempdir().unwrap();
        let store = StateStore::under(root.path());
        store.ensure_layout().unwrap();
        let config = config();
        let date = Utc::now().date_naive();
        let first_time = Utc::now();
        let first_api = FakeNetbird::new(&["alpha"], vec![Err(failure())]);
        let first = run_occurrence_step(&store, &config, &first_api, date, first_time)
            .await
            .unwrap();
        assert_eq!(first.outcome, JournalOutcome::InProgress);

        let retry_api = FakeNetbird::new(&["alpha"], vec![Ok(())]);
        let retry = run_occurrence_step(
            &store,
            &config,
            &retry_api,
            date,
            first_time + chrono::Duration::seconds(6),
        )
        .await
        .unwrap();
        assert_eq!(retry.outcome, JournalOutcome::Success);
        assert_eq!(&*retry_api.selections.lock().unwrap(), &["beta"]);

        let next_date = date.succ_opt().unwrap();
        let manual_api = FakeNetbird::new(&["alpha"], vec![Err(failure())]);
        run_occurrence_step(&store, &config, &manual_api, next_date, Utc::now())
            .await
            .unwrap();
        let superseding_api = FakeNetbird::new(&["manual"], vec![]);
        let superseded = run_occurrence_step(
            &store,
            &config,
            &superseding_api,
            next_date,
            Utc::now() + chrono::Duration::seconds(6),
        )
        .await
        .unwrap();
        assert_eq!(superseded.outcome, JournalOutcome::Superseded);
        assert!(superseding_api.selections.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn retries_are_bounded_and_raw_secrets_never_reach_state() {
        let root = tempdir().unwrap();
        let store = StateStore::under(root.path());
        store.ensure_layout().unwrap();
        let config = config();
        let date = Utc::now().date_naive();
        let api = FakeNetbird::new(
            &["alpha", "alpha", "alpha"],
            vec![Err(failure()), Err(failure()), Err(failure())],
        );
        let mut now = Utc::now();
        for delay in [0, 6, 31] {
            now += chrono::Duration::seconds(delay);
            run_occurrence_step(&store, &config, &api, date, now)
                .await
                .unwrap();
        }
        let journal = store.read_journal().unwrap().unwrap();
        assert_eq!(journal.outcome, JournalOutcome::Failed);
        assert_eq!(api.selections.lock().unwrap().len(), MAX_ATTEMPTS as usize);
        let persisted = std::fs::read_to_string(store.journal_path()).unwrap();
        assert!(!persisted.contains("token="));
        assert!(!persisted.contains("Authorization"));
    }

    #[tokio::test]
    async fn a_new_generation_cannot_execute_a_stale_retry_target() {
        let root = tempdir().unwrap();
        let store = StateStore::under(root.path());
        store.ensure_layout().unwrap();
        let old = config();
        let date = Utc::now().date_naive();
        store
            .write_journal(&ExecutionJournal {
                schema_version: SCHEMA_VERSION,
                generation: old.generation,
                local_date: date,
                original_profile: Some("alpha".into()),
                intended_target: Some("beta".into()),
                attempts: 1,
                outcome: JournalOutcome::InProgress,
                next_retry_at: None,
                last_error: Some(failure()),
                updated_at: Utc::now(),
            })
            .unwrap();
        let mut replacement = config();
        replacement.profiles = vec!["alpha".into(), "gamma".into()];
        let api = FakeNetbird::new(&["alpha"], vec![Ok(())]);
        let step = run_occurrence_step(&store, &replacement, &api, date, Utc::now())
            .await
            .unwrap();
        assert_eq!(step.outcome, JournalOutcome::Success);
        assert_eq!(&*api.selections.lock().unwrap(), &["gamma"]);
    }

    #[tokio::test]
    async fn a_terminal_occurrence_is_not_selected_twice() {
        let root = tempdir().unwrap();
        let store = StateStore::under(root.path());
        store.ensure_layout().unwrap();
        let config = config();
        let date = Utc::now().date_naive();
        let api = FakeNetbird::new(&["alpha"], vec![Ok(())]);
        run_occurrence_step(&store, &config, &api, date, Utc::now())
            .await
            .unwrap();
        run_occurrence_step(
            &store,
            &config,
            &api,
            date,
            Utc::now() + chrono::Duration::hours(1),
        )
        .await
        .unwrap();
        assert_eq!(api.selections.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn status_discovery_failure_recovers_on_a_bounded_retry() {
        let root = tempdir().unwrap();
        let store = StateStore::under(root.path());
        store.ensure_layout().unwrap();
        let config = config();
        let date = Utc::now().date_naive();
        let first_time = Utc::now();
        let api = FakeNetbird {
            statuses: Mutex::new(
                vec![Err(SanitizedError::timed_out("status")), Ok("alpha".into())].into(),
            ),
            selections: Mutex::new(Vec::new()),
            selection_results: Mutex::new(vec![Ok(())].into()),
        };
        let first = run_occurrence_step(&store, &config, &api, date, first_time)
            .await
            .unwrap();
        assert_eq!(first.outcome, JournalOutcome::PendingDiscovery);
        let recovered = run_occurrence_step(
            &store,
            &config,
            &api,
            date,
            first_time + chrono::Duration::seconds(6),
        )
        .await
        .unwrap();
        assert_eq!(recovered.outcome, JournalOutcome::Success);
        assert_eq!(&*api.selections.lock().unwrap(), &["beta"]);
    }
}
