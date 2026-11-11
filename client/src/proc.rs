//! Child process helpers: no console windows, and a Job Object so children
//! die together with the app (also on crash).

use std::ffi::OsStr;
use std::path::Path;

/// `CREATE_NO_WINDOW` process creation flag.
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A tokio command that never shows a console window.
pub fn command(program: &Path) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(program);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    cmd
}

/// A blocking std command that never shows a console window.
pub fn std_command(program: impl AsRef<OsStr>) -> std::process::Command {
    let mut cmd = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    cmd
}

/// Adds the child to a process-wide Job Object with KILL_ON_JOB_CLOSE, so it
/// is terminated by Windows when this process exits for any reason.
pub fn bind_to_job(child: &tokio::process::Child) {
    #[cfg(windows)]
    if let Some(h) = child.raw_handle() {
        job::assign(h);
    }
    #[cfg(not(windows))]
    let _ = child;
}

#[cfg(windows)]
mod job {
    use std::ffi::c_void;
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    struct Job(HANDLE);
    // SAFETY: a job handle can be used from any thread.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    fn job() -> Option<&'static Job> {
        static JOB: OnceLock<Option<Job>> = OnceLock::new();
        JOB.get_or_init(|| unsafe {
            let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if h.is_null() {
                return None;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                h,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                return None;
            }
            // The handle is intentionally never closed: the OS closes it when
            // the process ends, which kills every process in the job.
            Some(Job(h))
        })
        .as_ref()
    }

    pub fn assign(process: std::os::windows::io::RawHandle) {
        if let Some(j) = job() {
            let ok = unsafe { AssignProcessToJobObject(j.0, process as HANDLE) };
            if ok == 0 {
                tracing::warn!("AssignProcessToJobObject failed");
            }
        }
    }
}
