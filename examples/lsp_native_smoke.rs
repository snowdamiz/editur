use std::{
    process::Command as ProcessCommand,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use editur::lsp::{
    Command, Controller, DocumentSnapshot, Event, PresetId, RequestTag, ServerLaunch, ServerStatus,
    catalog,
};
use serde_json::json;

fn main() -> Result<(), String> {
    let requested = std::env::args().nth(1).ok_or_else(|| {
        "usage: cargo run --release --example lsp_native_smoke -- <rust-analyzer|typescript-language-server|pyright|gopls|clangd>".to_owned()
    })?;
    let preset = catalog()
        .iter()
        .find(|preset| preset.id.as_str() == requested)
        .ok_or_else(|| format!("unknown LSP preset: {requested}"))?;
    let (filename, language_id, text, feature_cursor, completion_cursor) = fixture(preset.id);
    let project = tempfile::tempdir().map_err(|error| error.to_string())?;
    let path = project.path().join(filename);
    std::fs::write(&path, text).map_err(|error| error.to_string())?;
    write_project_files(project.path(), preset.id)?;
    let controller = Controller::start(
        project.path().to_path_buf(),
        preset,
        ServerLaunch::Auto,
        Arc::new(|| {}),
    );
    controller.send(Command::Open(DocumentSnapshot {
        path: path.clone(),
        language_id: language_id.into(),
        text: text.into(),
        revision: 0,
    }))?;

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut ready = false;
    let mut next_features = Instant::now();
    let mut diagnostics = false;
    let mut completion = false;
    let mut hover = false;
    let mut definition = false;
    while Instant::now() < deadline && !(diagnostics && completion && hover && definition) {
        if ready && Instant::now() >= next_features {
            if !completion {
                controller.send(Command::Complete {
                    tag: tag(&path, completion_cursor),
                    trigger: None,
                })?;
            }
            if !hover {
                controller.send(Command::Hover(tag(&path, feature_cursor)))?;
            }
            if !definition {
                controller.send(Command::Definition(tag(&path, feature_cursor)))?;
            }
            next_features = Instant::now() + Duration::from_millis(750);
        }
        let event = match controller.events().recv_timeout(Duration::from_millis(100)) {
            Ok(event) => event,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(error) => return Err(format!("{requested} disconnected: {error}")),
        };
        match event {
            Event::StateChanged(ServerStatus::Ready(_)) => ready = true,
            Event::Diagnostics {
                diagnostics: found, ..
            } => diagnostics |= !found.is_empty(),
            Event::Completion { items, .. } => completion |= !items.is_empty(),
            Event::Hover { content, .. } => {
                hover |= content.is_some_and(|content| !content.text.is_empty())
            }
            Event::Definitions { locations, .. } => definition |= !locations.is_empty(),
            Event::ProcessExited { error, stderr } => {
                return Err(format!("{requested} exited: {error}\n{stderr}"));
            }
            _ => {}
        }
    }
    if !(diagnostics && completion && hover && definition) {
        return Err(format!(
            "{requested} smoke incomplete: diagnostics={diagnostics}, completion={completion}, hover={hover}, definition={definition}"
        ));
    }
    controller.send(Command::Restart(ServerLaunch::Custom {
        command: preset.command.into(),
        args: preset
            .args
            .iter()
            .map(|argument| (*argument).into())
            .collect(),
    }))?;
    wait_for_status(&controller, requested.as_str(), true)?;
    controller.send(Command::Restart(ServerLaunch::Off))?;
    wait_for_status(&controller, requested.as_str(), false)?;
    controller.send(Command::Restart(ServerLaunch::Auto))?;
    wait_for_status(&controller, requested.as_str(), true)?;
    controller.send(Command::Close(path))?;
    wait_for_status(&controller, requested.as_str(), false)?;
    let version = server_version(preset.id, preset.command)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "recordedAtUnixSeconds": SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "preset": preset.id.as_str(),
            "serverVersion": version,
            "diagnostics": true,
            "completion": true,
            "completionUndo": "manual UI check required",
            "hover": true,
            "definition": true,
            "customRestartDisableReenable": "protocol passed; manual UI check required",
            "descendantFreeShutdown": "manual process-tree check required"
        }))
        .map_err(|error| error.to_string())?
    );
    Ok(())
}

fn wait_for_status(controller: &Controller, preset: &str, ready: bool) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        match controller.events().recv_timeout(Duration::from_millis(250)) {
            Ok(Event::StateChanged(ServerStatus::Ready(_))) if ready => return Ok(()),
            Ok(Event::StateChanged(ServerStatus::Stopped)) if !ready => return Ok(()),
            Ok(Event::ProcessExited { error, stderr }) => {
                return Err(format!("{preset} exited: {error}\n{stderr}"));
            }
            Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(error) => return Err(format!("{preset} disconnected: {error}")),
        }
    }
    Err(format!(
        "{preset} did not become {}",
        if ready { "ready" } else { "stopped" }
    ))
}

fn server_version(id: PresetId, default_command: &str) -> Result<String, String> {
    let (command, argument) = match id {
        PresetId::Pyright => ("pyright", "--version"),
        PresetId::Gopls => (default_command, "version"),
        _ => (default_command, "--version"),
    };
    let output = ProcessCommand::new(command)
        .arg(argument)
        .output()
        .map_err(|error| format!("cannot query {command} version: {error}"))?;
    let bytes = if output.stdout.is_empty() {
        &output.stderr
    } else {
        &output.stdout
    };
    let output_text = String::from_utf8_lossy(bytes);
    let version = output_text
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(str::trim)
        .unwrap_or_default();
    if !output.status.success() || version.is_empty() {
        return Err(format!("cannot query {command} version: {version}"));
    }
    Ok(version.to_owned())
}

fn write_project_files(root: &std::path::Path, id: PresetId) -> Result<(), String> {
    let file = match id {
        PresetId::RustAnalyzer => Some((
            "Cargo.toml",
            "[package]\nname = \"editur-lsp-smoke\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[[bin]]\nname = \"smoke\"\npath = \"main.rs\"\n",
        )),
        PresetId::TypeScript => Some((
            "tsconfig.json",
            "{\"compilerOptions\":{\"strict\":true},\"files\":[\"main.ts\"]}\n",
        )),
        PresetId::Pyright => Some(("pyrightconfig.json", "{\"include\":[\"main.py\"]}\n")),
        PresetId::Gopls => Some(("go.mod", "module editur-lsp-smoke\n\ngo 1.20\n")),
        PresetId::Clangd => Some(("compile_flags.txt", "-std=c11\n")),
    };
    if let Some((name, contents)) = file {
        std::fs::write(root.join(name), contents).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn tag(path: &std::path::Path, cursor: usize) -> RequestTag {
    RequestTag {
        path: path.to_path_buf(),
        revision: 0,
        cursor,
    }
}

fn fixture(id: PresetId) -> (&'static str, &'static str, &'static str, usize, usize) {
    let (filename, language, text) = match id {
        PresetId::RustAnalyzer => (
            "main.rs",
            "rust",
            "fn answer() -> i32 { 42 }\nfn main() { println!(\"{}\", answer()); let broken: i32 = ; }\n",
        ),
        PresetId::TypeScript => (
            "main.ts",
            "typescript",
            "function answer(): number { return 42; }\nanswer();\nconst broken: number = ;\n",
        ),
        PresetId::Pyright => (
            "main.py",
            "python",
            "def answer() -> int:\n    return 42\n\nanswer()\nbroken =\n",
        ),
        PresetId::Gopls => (
            "main.go",
            "go",
            "package main\nfunc answer() int { return 42 }\nfunc main() { println(answer()); var broken int = }\n",
        ),
        PresetId::Clangd => (
            "main.c",
            "c",
            "int answer(void) { return 42; }\nint main(void) { int value = answer(); int broken = ; return value; }\n",
        ),
    };
    let feature_cursor = text.rfind("answer").unwrap() + 1;
    let completion_cursor = text.rfind("answer").unwrap();
    (filename, language, text, feature_cursor, completion_cursor)
}
