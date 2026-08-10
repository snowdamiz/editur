use std::{env, ffi::OsString, process::ExitCode, time::Instant};

use editur::{
    app,
    cli::{Command, parse_args},
    data_dir,
    file_io::resolve_target,
};

const HELP: &str = "Editur — a small native editor for quick file changes

Usage:
  editur [PATH]
  editur update
  editur --help
  editur --version";

fn main() -> ExitCode {
    let started = Instant::now();
    match run(env::args_os().skip(1).collect(), started) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("editur: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<OsString>, started: Instant) -> Result<(), String> {
    match parse_args(args)? {
        Command::Help => {
            println!("{HELP}");
            Ok(())
        }
        Command::Version => {
            println!("editur {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Command::Open(path) => {
            let cwd = env::current_dir()
                .map_err(|error| format!("cannot determine current directory: {error}"))?;
            let target = resolve_target(&cwd, path.as_deref())?;
            if env::var("EDITUR_LOG").as_deref() == Ok("debug") {
                eprintln!("editur: path ready in {:.2?}", started.elapsed());
            }
            app::launch(target, started)
        }
        Command::Resident(path) => {
            let cwd = env::current_dir()
                .map_err(|error| format!("cannot determine current directory: {error}"))?;
            app::run(resolve_target(&cwd, Some(&path))?, started)
        }
        Command::QuitRunning => app::quit_running(),
        Command::AgentProcess(provider, project_root, extra_args) => {
            editur::agent::run_managed_process(provider, &project_root, extra_args)
        }
        Command::AgentProvision(provider) => {
            let bundle = editur::agent::provision::embedded_bundle()?;
            let manifest = bundle.manifest(provider)?;
            let sidecar = editur::agent::provision::ensure(manifest, &data_dir()?, |progress| {
                if let Some(total) = progress.total {
                    eprint!(
                        "\rProvisioning {}: {}/{} MiB",
                        editur::agent::provider::descriptor(provider).display_name,
                        progress.downloaded / 1_048_576,
                        total / 1_048_576
                    );
                }
            })?;
            println!(
                "\nProvisioned {} {}.",
                editur::agent::provider::descriptor(provider).display_name,
                sidecar.version
            );
            Ok(())
        }
        Command::Update => editur::update::run(),
        #[cfg(windows)]
        Command::FinishUpdate(destination) => editur::update::finish_windows(&destination),
        #[cfg(windows)]
        Command::CleanupUpdate(temporary) => editur::update::cleanup_windows(&temporary),
    }
}
