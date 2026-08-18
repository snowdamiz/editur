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

pub fn data_dir() -> Result<std::path::PathBuf, String> {
    directories::ProjectDirs::from("io", "editur", "Editur")
        .map(|directories| directories.data_dir().to_path_buf())
        .ok_or_else(|| "cannot determine the application data directory".to_owned())
}
