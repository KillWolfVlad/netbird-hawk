use std::{
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};

use directories::ProjectDirs;
use fs2::FileExt;
use serde::{Serialize, de::DeserializeOwned};
use tempfile::NamedTempFile;

use crate::{
    error::{HawkError, Result},
    model::{ControlIntent, DesiredConfig, ExecutionJournal, RuntimeSnapshot},
};

const APP_NAME: &str = "netbird-hawk";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub state_dir: PathBuf,
    pub log_dir: PathBuf,
}

impl AppPaths {
    pub fn from_system() -> Result<Self> {
        if let Some(root) = std::env::var_os("NETBIRD_HAWK_HOME") {
            let root = PathBuf::from(root);
            if !root.is_absolute() {
                return Err(HawkError::Validation(
                    "NETBIRD_HAWK_HOME must be an absolute path".to_owned(),
                ));
            }
            return Ok(Self::under(root));
        }
        let dirs =
            ProjectDirs::from("", "", APP_NAME).ok_or(HawkError::StateDirectoryUnavailable)?;
        Ok(Self {
            config_dir: dirs.config_dir().to_path_buf(),
            state_dir: dirs
                .state_dir()
                .unwrap_or_else(|| dirs.data_local_dir())
                .to_path_buf(),
            log_dir: dirs.data_local_dir().join("logs"),
        })
    }

    pub fn under(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref();
        Self {
            config_dir: root.join("config"),
            state_dir: root.join("state"),
            log_dir: root.join("logs"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct StateStore {
    paths: AppPaths,
}

impl StateStore {
    pub fn from_system() -> Result<Self> {
        Ok(Self::new(AppPaths::from_system()?))
    }

    pub fn new(paths: AppPaths) -> Self {
        Self { paths }
    }

    pub fn under(root: impl AsRef<Path>) -> Self {
        Self::new(AppPaths::under(root))
    }

    pub fn paths(&self) -> &AppPaths {
        &self.paths
    }

    pub fn ensure_layout(&self) -> Result<()> {
        for path in [
            &self.paths.config_dir,
            &self.paths.state_dir,
            &self.paths.log_dir,
        ] {
            fs::create_dir_all(path)
                .map_err(|source| HawkError::io("create directory", path, source))?;
            restrict_directory(path)?;
        }
        Ok(())
    }

    pub fn config_path(&self) -> PathBuf {
        self.paths.config_dir.join("config.json")
    }

    pub fn control_path(&self) -> PathBuf {
        self.paths.state_dir.join("control.json")
    }

    pub fn journal_path(&self) -> PathBuf {
        self.paths.state_dir.join("journal.json")
    }

    pub fn runtime_path(&self) -> PathBuf {
        self.paths.state_dir.join("runtime.json")
    }

    fn controller_lock_path(&self) -> PathBuf {
        self.paths.state_dir.join("controller.lock")
    }

    fn lifetime_lock_path(&self) -> PathBuf {
        self.paths.state_dir.join("daemon.lock")
    }

    pub fn read_config(&self) -> Result<Option<DesiredConfig>> {
        let value: Option<DesiredConfig> = read_json(&self.config_path(), "desired configuration")?;
        if let Some(value) = &value {
            value.validate()?;
        }
        Ok(value)
    }

    pub fn write_config(&self, value: &DesiredConfig) -> Result<()> {
        value.validate()?;
        atomic_write_json(&self.config_path(), value)
    }

    pub fn read_control(&self) -> Result<Option<ControlIntent>> {
        let value: Option<ControlIntent> = read_json(&self.control_path(), "control intent")?;
        if let Some(value) = &value {
            value.validate()?;
        }
        Ok(value)
    }

    pub fn write_control(&self, value: &ControlIntent) -> Result<()> {
        value.validate()?;
        atomic_write_json(&self.control_path(), value)
    }

    pub fn read_journal(&self) -> Result<Option<ExecutionJournal>> {
        let value: Option<ExecutionJournal> = read_json(&self.journal_path(), "execution journal")?;
        if let Some(value) = &value {
            value.validate()?;
        }
        Ok(value)
    }

    pub fn write_journal(&self, value: &ExecutionJournal) -> Result<()> {
        value.validate()?;
        atomic_write_json(&self.journal_path(), value)
    }

    pub fn read_runtime(&self) -> Result<Option<RuntimeSnapshot>> {
        let value: Option<RuntimeSnapshot> = read_json(&self.runtime_path(), "runtime snapshot")?;
        if let Some(value) = &value {
            value.validate()?;
        }
        Ok(value)
    }

    pub fn write_runtime(&self, value: &RuntimeSnapshot) -> Result<()> {
        value.validate()?;
        atomic_write_json(&self.runtime_path(), value)
    }

    pub fn controller_lock(&self) -> Result<FileLock> {
        let path = self.controller_lock_path();
        let file = open_private_lock_file(&path)?;
        file.lock_exclusive()
            .map_err(|source| HawkError::io("lock controller operations", &path, source))?;
        Ok(FileLock { file })
    }

    pub fn try_lifetime_lock(&self) -> Result<Option<FileLock>> {
        let path = self.lifetime_lock_path();
        let file = open_private_lock_file(&path)?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(FileLock { file })),
            Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(None),
            Err(source) => Err(HawkError::io("lock daemon lifetime", path, source)),
        }
    }

    pub fn daemon_is_live(&self) -> Result<bool> {
        match self.try_lifetime_lock()? {
            Some(lock) => {
                drop(lock);
                Ok(false)
            }
            None => Ok(true),
        }
    }
}

#[derive(Debug)]
pub struct FileLock {
    file: File,
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn read_json<T: DeserializeOwned>(path: &Path, record: &'static str) -> Result<Option<T>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(HawkError::io("read state", path, source)),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|source| HawkError::InvalidState { record, source })
}

fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        HawkError::Validation(format!("state path has no parent: {}", path.display()))
    })?;
    fs::create_dir_all(parent)
        .map_err(|source| HawkError::io("create state directory", parent, source))?;
    restrict_directory(parent)?;

    let mut temporary = NamedTempFile::new_in(parent)
        .map_err(|source| HawkError::io("create temporary state", parent, source))?;
    restrict_file(temporary.as_file(), temporary.path())?;
    serde_json::to_writer(&mut temporary, value).map_err(|source| HawkError::InvalidState {
        record: "state record",
        source,
    })?;
    temporary
        .write_all(b"\n")
        .map_err(|source| HawkError::io("write temporary state", temporary.path(), source))?;
    temporary
        .flush()
        .map_err(|source| HawkError::io("flush temporary state", temporary.path(), source))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|source| HawkError::io("sync temporary state", temporary.path(), source))?;
    temporary
        .persist(path)
        .map_err(|error| HawkError::io("replace state atomically", path, error.error))?;
    restrict_path_file(path)?;
    sync_directory(parent)?;
    Ok(())
}

fn open_private_lock_file(path: &Path) -> Result<File> {
    let parent = path.parent().ok_or_else(|| {
        HawkError::Validation(format!("lock path has no parent: {}", path.display()))
    })?;
    fs::create_dir_all(parent)
        .map_err(|source| HawkError::io("create lock directory", parent, source))?;
    restrict_directory(parent)?;

    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(path)
        .map_err(|source| HawkError::io("open lock", path, source))?;
    restrict_file(&file, path)?;
    Ok(file)
}

#[cfg(unix)]
fn restrict_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|source| HawkError::io("set directory permissions", path, source))
}

#[cfg(not(unix))]
fn restrict_directory(_path: &Path) -> Result<()> {
    // User-profile ACLs are inherited on Windows. Do not broaden or replace them.
    Ok(())
}

#[cfg(unix)]
fn restrict_file(file: &File, path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|source| HawkError::io("set file permissions", path, source))
}

#[cfg(not(unix))]
fn restrict_file(_file: &File, _path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn restrict_path_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|source| HawkError::io("set file permissions", path, source))
}

#[cfg(not(unix))]
fn restrict_path_file(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| HawkError::io("sync state directory", path, source))
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use tempfile::tempdir;

    use crate::model::{DesiredRunState, SCHEMA_VERSION};

    use super::*;

    #[test]
    fn round_trips_atomic_control_record_and_ignores_interrupted_temp_file() {
        let root = tempdir().unwrap();
        let store = StateStore::under(root.path());
        store.ensure_layout().unwrap();
        let control = ControlIntent {
            schema_version: SCHEMA_VERSION,
            desired: DesiredRunState::Running,
            generation: None,
            requested_at: Utc::now(),
        };
        store.write_control(&control).unwrap();
        fs::write(store.paths.state_dir.join(".interrupted.tmp"), b"{").unwrap();
        assert_eq!(store.read_control().unwrap(), Some(control));
    }

    #[test]
    fn rejects_unsupported_versions() {
        let root = tempdir().unwrap();
        let store = StateStore::under(root.path());
        store.ensure_layout().unwrap();
        fs::write(
            store.control_path(),
            r#"{"schema_version":99,"desired":"stopped","generation":null,"requested_at":"2025-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        assert!(matches!(
            store.read_control(),
            Err(HawkError::UnsupportedVersion { found: 99, .. })
        ));
    }

    #[test]
    fn lifetime_lock_is_the_liveness_truth_even_with_stale_runtime_metadata() {
        let root = tempdir().unwrap();
        let store = StateStore::under(root.path());
        store.ensure_layout().unwrap();
        store.write_runtime(&RuntimeSnapshot::stopped()).unwrap();
        assert!(!store.daemon_is_live().unwrap());
        let lock = store.try_lifetime_lock().unwrap().unwrap();
        assert!(store.daemon_is_live().unwrap());
        drop(lock);
        assert!(!store.daemon_is_live().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn uses_private_unix_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempdir().unwrap();
        let store = StateStore::under(root.path());
        store.ensure_layout().unwrap();
        assert_eq!(
            fs::metadata(&store.paths.state_dir)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}
