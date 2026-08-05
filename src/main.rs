use std::{env, ffi::OsString, process::ExitCode, time::Instant};

use editur::{
    app,
    cli::{Command, SyntaxCommand, parse_args},
    file_io::resolve_target,
    syntax::{data_dir, package::PackageManager},
};

const HELP: &str = "Editur — a small native editor for quick file changes

Usage:
  editur [PATH]
  editur syntax list
  editur syntax install <LANGUAGE|PACKAGE>
  editur syntax remove <LANGUAGE>
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
            app::launch(target)
        }
        Command::Resident(path) => {
            let cwd = env::current_dir()
                .map_err(|error| format!("cannot determine current directory: {error}"))?;
            app::run(resolve_target(&cwd, Some(&path))?, started)
        }
        Command::Syntax(command) => syntax_command(command),
        Command::Update => editur::update::run(),
        #[cfg(windows)]
        Command::FinishUpdate(destination) => editur::update::finish_windows(&destination),
        #[cfg(windows)]
        Command::CleanupUpdate(temporary) => editur::update::cleanup_windows(&temporary),
    }
}

fn syntax_command(command: SyntaxCommand) -> Result<(), String> {
    let manager = PackageManager::new(data_dir()?);
    match command {
        SyntaxCommand::List => {
            println!("Installed:\n  rust (built in)");
            for manifest in manager.installed()? {
                println!("  {} {}", manifest.id, manifest.version);
            }
            let catalog_url = env::var("EDITUR_SYNTAX_CATALOG")
                .unwrap_or_else(|_| editur::syntax::package::OFFICIAL_CATALOG.to_owned());
            let catalog = PackageManager::fetch_catalog(&catalog_url)?;
            println!("\nAvailable:");
            for package in catalog.packages {
                if !manager.package_dir(&package.id).is_dir() {
                    println!("  {}", package.id);
                }
            }
            Ok(())
        }
        SyntaxCommand::Install(source) => {
            let manifest = manager.install(&source)?;
            println!("Installed {} {}", manifest.id, manifest.version);
            Ok(())
        }
        SyntaxCommand::Remove(id) => {
            manager.remove(&id)?;
            println!("Removed {id}");
            Ok(())
        }
    }
}
