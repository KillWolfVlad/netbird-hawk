use std::{
    path::Path,
    process::{Command, Stdio},
};

use crate::error::{HawkError, Result};

pub trait ProcessLauncher: Send + Sync {
    fn launch_worker(&self, current_executable: &Path) -> Result<u32>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DetachedProcessLauncher;

impl ProcessLauncher for DetachedProcessLauncher {
    fn launch_worker(&self, current_executable: &Path) -> Result<u32> {
        let mut command = Command::new(current_executable);
        command
            .arg("__worker")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_detached(&mut command);
        command
            .spawn()
            .map(|child| child.id())
            .map_err(|error| HawkError::Launch(error.to_string()))
    }
}

#[cfg(unix)]
fn configure_detached(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    // SAFETY: setsid is async-signal-safe and the closure touches no shared Rust
    // state between fork and exec.
    unsafe {
        command.pre_exec(|| {
            nix::unistd::setsid()
                .map(|_| ())
                .map_err(std::io::Error::other)
        });
    }
}

#[cfg(windows)]
fn configure_detached(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::{
        CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, DETACHED_PROCESS,
    };

    command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW | DETACHED_PROCESS);
}

#[cfg(not(any(unix, windows)))]
compile_error!("netbird-hawk supports Unix-family platforms and Windows");

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Debug, Default)]
    pub struct FakeProcessLauncher {
        pub launches: Mutex<Vec<std::path::PathBuf>>,
    }

    impl ProcessLauncher for FakeProcessLauncher {
        fn launch_worker(&self, current_executable: &Path) -> Result<u32> {
            self.launches
                .lock()
                .unwrap()
                .push(current_executable.to_path_buf());
            Ok(42)
        }
    }

    #[test]
    fn process_launcher_boundary_is_injectable() {
        let launcher = FakeProcessLauncher::default();
        launcher
            .launch_worker(Path::new("/absolute/netbird-hawk"))
            .unwrap();
        assert_eq!(launcher.launches.lock().unwrap().len(), 1);
    }
}
