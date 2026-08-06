pub mod controller;
pub mod provision;
pub mod state;

#[cfg(windows)]
pub(crate) const WINDOWS_JOB_ENV: &str = "EDITUR_AGENT_JOB";

#[cfg(windows)]
#[doc(hidden)]
pub struct WindowsJob {
    _handle: windows::core::Owned<windows::Win32::Foundation::HANDLE>,
}

#[cfg(windows)]
#[doc(hidden)]
pub fn new_windows_job() -> Result<(String, WindowsJob), String> {
    use std::os::windows::ffi::OsStrExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use windows::Win32::System::JobObjects::{
        CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject,
    };
    use windows::core::{Owned, PCWSTR};

    static NEXT_JOB: AtomicU64 = AtomicU64::new(1);
    let name = format!(
        "EditurAgent-{}-{}",
        std::process::id(),
        NEXT_JOB.fetch_add(1, Ordering::Relaxed)
    );
    let wide = std::ffi::OsStr::new(&name)
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let handle = unsafe {
        Owned::new(
            CreateJobObjectW(None, PCWSTR(wide.as_ptr()))
                .map_err(|error| format!("cannot create Cursor Agent process job: {error}"))?,
        )
    };
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    unsafe {
        SetInformationJobObject(
            *handle,
            JobObjectExtendedLimitInformation,
            std::ptr::from_ref(&limits).cast(),
            u32::try_from(std::mem::size_of_val(&limits))
                .map_err(|_| "Cursor Agent process job limits are too large")?,
        )
        .map_err(|error| format!("cannot configure Cursor Agent process job: {error}"))?;
    }
    Ok((name, WindowsJob { _handle: handle }))
}

#[cfg(windows)]
#[doc(hidden)]
pub fn join_windows_job(name: &str) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::System::{
        JobObjects::{AssignProcessToJobObject, OpenJobObjectW},
        SystemServices::JOB_OBJECT_ASSIGN_PROCESS,
        Threading::GetCurrentProcess,
    };
    use windows::core::{Owned, PCWSTR};

    let wide = std::ffi::OsStr::new(name)
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let handle = unsafe {
        Owned::new(
            OpenJobObjectW(JOB_OBJECT_ASSIGN_PROCESS, false, PCWSTR(wide.as_ptr()))
                .map_err(|error| format!("cannot open Cursor Agent process job: {error}"))?,
        )
    };
    unsafe {
        AssignProcessToJobObject(*handle, GetCurrentProcess())
            .map_err(|error| format!("cannot contain Cursor Agent process tree: {error}"))?;
    }
    Ok(())
}

pub fn run_managed_process(project_root: &std::path::Path) -> Result<(), String> {
    let project_root = std::fs::canonicalize(project_root).map_err(|error| {
        format!(
            "cannot open agent workspace {}: {error}",
            project_root.display()
        )
    })?;
    if !project_root.is_dir() {
        return Err(format!(
            "agent workspace is not a directory: {}",
            project_root.display()
        ));
    }
    let data_dir = crate::syntax::data_dir()?;
    let manifest = provision::embedded_manifest()?;
    let sidecar = provision::installed(&manifest, &data_dir)?;
    #[cfg(windows)]
    join_windows_job(
        &std::env::var(WINDOWS_JOB_ENV)
            .map_err(|_| "Cursor Agent process job was not supplied".to_owned())?,
    )?;
    let mut process = std::process::Command::new(&sidecar.command);
    process
        .args(&sidecar.args)
        .current_dir(&project_root)
        .env("CURSOR_INVOKED_AS", "cursor-agent")
        .env("NODE_COMPILE_CACHE", data_dir.join("agents/cursor/cache"));
    #[cfg(windows)]
    process.env_remove(WINDOWS_JOB_ENV);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;

        let error = process.exec();
        Err(format!("cannot start managed Cursor Agent: {error}"))
    }
    #[cfg(windows)]
    {
        let status = process
            .status()
            .map_err(|error| format!("cannot start managed Cursor Agent: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("managed Cursor Agent exited with {status}"))
        }
    }
    #[cfg(not(any(unix, windows)))]
    Err("managed Cursor Agent is unsupported on this operating system".into())
}
