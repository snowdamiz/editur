pub mod controller;
pub mod provider;
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
impl WindowsJob {
    pub(crate) fn assign_child(&self, child: &std::process::Child) -> Result<(), String> {
        use std::os::windows::io::AsRawHandle as _;
        use windows::Win32::{Foundation::HANDLE, System::JobObjects::AssignProcessToJobObject};

        unsafe {
            AssignProcessToJobObject(*self._handle, HANDLE(child.as_raw_handle()))
                .map_err(|error| format!("cannot contain child process tree: {error}"))
        }
    }
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
                .map_err(|error| format!("cannot create ACP provider process job: {error}"))?,
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
                .map_err(|_| "ACP provider process job limits are too large")?,
        )
        .map_err(|error| format!("cannot configure ACP provider process job: {error}"))?;
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
                .map_err(|error| format!("cannot open ACP provider process job: {error}"))?,
        )
    };
    unsafe {
        AssignProcessToJobObject(*handle, GetCurrentProcess())
            .map_err(|error| format!("cannot contain ACP provider process tree: {error}"))?;
    }
    Ok(())
}

pub fn run_managed_process(
    provider: provider::ProviderId,
    project_root: &std::path::Path,
    extra_args: Vec<std::ffi::OsString>,
) -> Result<(), String> {
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
    let data_dir = crate::data_dir()?;
    let bundle = provision::embedded_bundle()?;
    let installed = provision::installed(bundle.manifest(provider)?, &data_dir)?;
    let prepared = provider::prepare_installed(provider, installed, &data_dir);
    #[cfg(windows)]
    join_windows_job(
        &std::env::var(WINDOWS_JOB_ENV)
            .map_err(|_| "agent process job was not supplied".to_owned())?,
    )?;
    let mut process = std::process::Command::new(&prepared.command);
    process
        .args(&prepared.args)
        .args(extra_args)
        .current_dir(&project_root)
        .envs(prepared.env.iter().map(|(name, value)| (name, value)));
    for name in prepared.remove_env {
        process.env_remove(name);
    }
    #[cfg(windows)]
    process.env_remove(WINDOWS_JOB_ENV);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;

        let error = process.exec();
        Err(format!(
            "cannot start managed {}: {error}",
            prepared.display_name
        ))
    }
    #[cfg(windows)]
    {
        let status = process
            .status()
            .map_err(|error| format!("cannot start managed {}: {error}", prepared.display_name))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!(
                "managed {} exited with {status}",
                prepared.display_name
            ))
        }
    }
    #[cfg(not(any(unix, windows)))]
    Err(format!(
        "managed {} is unsupported on this operating system",
        prepared.display_name
    ))
}
