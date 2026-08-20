pub mod agent;
pub mod app;
pub mod buffer;
pub mod cli;
mod components;
pub mod devin;
mod dialog;
pub mod editor_surface;
pub mod file_io;
pub mod git;
mod icons;
mod instance;
pub mod keybindings;
pub mod lsp;
mod markdown;
mod network;
mod pane;
pub mod projects;
pub mod renderer;
mod scrollbar;
pub mod search;
pub mod settings;
pub mod syntax;
mod terminal;
mod theme;
mod toast;
pub mod tree;
pub mod tree_surface;
pub mod update;
pub mod vim;

#[cfg(unix)]
pub(crate) fn configure_process_tree(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt as _;
    command.process_group(0);
}

#[cfg(not(unix))]
pub(crate) fn configure_process_tree(_command: &mut std::process::Command) {}

#[cfg(unix)]
pub(crate) fn terminate_process_tree(child: &mut std::process::Child) {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    unsafe {
        kill(-(child.id() as i32), 9);
    }
    let _ = child.kill();
}

#[cfg(not(unix))]
pub(crate) fn terminate_process_tree(child: &mut std::process::Child) {
    let _ = child.kill();
}

pub(crate) fn reap_worker(name: &'static str, worker: std::thread::JoinHandle<()>) {
    let _ = std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let _ = worker.join();
        });
}

pub fn data_dir() -> Result<std::path::PathBuf, String> {
    directories::ProjectDirs::from("io", "editur", "Editur")
        .map(|directories| directories.data_dir().to_path_buf())
        .ok_or_else(|| "cannot determine the application data directory".to_owned())
}
