use std::path::Path;

#[cfg(unix)]
use std::process::{Command, Stdio};

use crate::error::{HawkError, Result};

pub trait ProcessLauncher: Send + Sync {
    fn launch_worker(&self, current_executable: &Path) -> Result<u32>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DetachedProcessLauncher;

impl ProcessLauncher for DetachedProcessLauncher {
    fn launch_worker(&self, current_executable: &Path) -> Result<u32> {
        launch_worker(current_executable).map_err(|error| HawkError::Launch(error.to_string()))
    }
}

#[cfg(unix)]
fn launch_worker(current_executable: &Path) -> std::io::Result<u32> {
    use std::os::unix::process::CommandExt;

    let mut command = Command::new(current_executable);
    command
        .arg("__worker")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: setsid is async-signal-safe and the closure touches no shared Rust
    // state between fork and exec.
    unsafe {
        command.pre_exec(|| {
            nix::unistd::setsid()
                .map(|_| ())
                .map_err(std::io::Error::other)
        });
    }
    command.spawn().map(|child| child.id())
}

#[cfg(windows)]
fn launch_worker(current_executable: &Path) -> std::io::Result<u32> {
    use std::{ffi::OsStr, mem::size_of, os::windows::ffi::OsStrExt, ptr::null};

    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{
            CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, CreateProcessW, DETACHED_PROCESS,
            PROCESS_INFORMATION, STARTUPINFOW,
        },
    };

    if !current_executable.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "worker executable path must be absolute",
        ));
    }

    let mut application_name: Vec<u16> = current_executable.as_os_str().encode_wide().collect();
    if application_name.contains(&0) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "worker executable path contains a NUL code unit",
        ));
    }
    application_name.push(0);

    // argv[0] is deliberately neutral and the only argument is the fixed,
    // internal worker entry point. No user-controlled value reaches parsing.
    let mut command_line: Vec<u16> = OsStr::new("netbird-hawk __worker")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let startup_info = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut process_information = PROCESS_INFORMATION::default();

    // SAFETY: both UTF-16 buffers are owned, NUL-terminated, and live for the
    // duration of the call. The command-line buffer is writable as required by
    // CreateProcessW. Optional security, environment, and directory pointers
    // are null, and both output handles are closed below on every success.
    let created = unsafe {
        CreateProcessW(
            application_name.as_ptr(),
            command_line.as_mut_ptr(),
            null(),
            null(),
            0,
            CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW | DETACHED_PROCESS,
            null(),
            null(),
            &startup_info,
            &mut process_information,
        )
    };
    if created == 0 {
        return Err(std::io::Error::last_os_error());
    }

    debug_assert_ne!(process_information.dwProcessId, 0);
    debug_assert!(!process_information.hProcess.is_null());
    debug_assert!(!process_information.hThread.is_null());
    let process_id = process_information.dwProcessId;
    // SAFETY: CreateProcessW succeeded, so PROCESS_INFORMATION owns both valid
    // handles. They are not retained or closed anywhere else.
    let process_closed = unsafe { CloseHandle(process_information.hProcess) };
    // SAFETY: same ownership argument as for hProcess above.
    let thread_closed = unsafe { CloseHandle(process_information.hThread) };
    debug_assert_ne!(process_closed, 0);
    debug_assert_ne!(thread_closed, 0);

    Ok(process_id)
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

    #[cfg(windows)]
    #[test]
    fn windows_launcher_requires_an_absolute_executable() {
        let error = launch_worker(Path::new("netbird-hawk.exe")).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[cfg(windows)]
    #[test]
    fn windows_launcher_preserves_the_win32_creation_error() {
        let root = tempfile::tempdir().unwrap();
        let error = launch_worker(&root.path().join("missing-worker.exe")).unwrap_err();
        assert!(error.raw_os_error().is_some());
        assert!(matches!(
            DetachedProcessLauncher.launch_worker(&root.path().join("missing-worker.exe")),
            Err(HawkError::Launch(message)) if !message.is_empty()
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_launcher_returns_the_created_process_id() {
        let process_id = launch_worker(&std::env::current_exe().unwrap()).unwrap();
        assert_ne!(process_id, 0);
    }
}
