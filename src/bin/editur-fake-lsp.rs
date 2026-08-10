use std::{
    fs::OpenOptions,
    io::{BufReader, Write},
    path::PathBuf,
    process::{Command, Stdio},
    time::Duration,
};

use serde_json::{Value, json};

#[derive(Default)]
struct Options {
    record: Option<PathBuf>,
    diagnostics: bool,
    versionless_diagnostics: bool,
    stale_diagnostics: bool,
    delay_features: bool,
    delay_initialize: bool,
    feature_errors: bool,
    one_definition: bool,
    full_sync: bool,
    missing_sync: bool,
    split_writes: bool,
    server_requests: bool,
    malformed: bool,
    oversized: bool,
    exit_on_open: bool,
    late_response: bool,
    stderr_lines: usize,
    descendant_pid: Option<PathBuf>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fake LSP server: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let options = parse_options()?;
    for line in 0..options.stderr_lines {
        eprintln!("fake stderr line {line:06}: {}", "x".repeat(96));
    }
    if let Some(path) = &options.descendant_pid {
        let child = Command::new(std::env::current_exe().map_err(|error| error.to_string())?)
            .arg("--hang")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("cannot spawn descendant: {error}"))?;
        std::fs::write(path, child.id().to_string()).map_err(|error| error.to_string())?;
    }

    let mut delayed_completion = None;
    let mut delayed_hover = None;
    let mut delayed_definition = None;
    let mut opened_document = None;
    let mut input = BufReader::new(std::io::stdin().lock());
    let mut output = std::io::stdout().lock();
    loop {
        let body = editur::lsp::read_frame(&mut input)?;
        let message: Value = serde_json::from_slice(&body)
            .map_err(|error| format!("invalid client JSON: {error}"))?;
        record(&options.record, &message)?;
        let method = message.get("method").and_then(Value::as_str);
        let id = message.get("id").cloned();
        match (method, id) {
            (Some("initialize"), Some(id)) => {
                if options.delay_initialize {
                    std::thread::sleep(Duration::from_millis(200));
                }
                let sync = if options.missing_sync {
                    Value::Null
                } else if options.full_sync {
                    json!({"openClose": true, "change": 1, "save": true})
                } else {
                    json!({"openClose": true, "change": 2, "save": {"includeText": true}})
                };
                let mut capabilities = json!({
                    "completionProvider": {"triggerCharacters": ["."]},
                    "hoverProvider": true,
                    "definitionProvider": true
                });
                if !options.missing_sync {
                    capabilities["textDocumentSync"] = sync;
                }
                send_frame(
                    &mut output,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {"capabilities": capabilities}
                    }),
                    options.split_writes,
                )?;
            }
            (Some("initialized"), None) if options.server_requests => {
                send_frame(
                    &mut output,
                    &json!({
                        "jsonrpc": "2.0",
                        "method": "fake/unknownNotification",
                        "params": {}
                    }),
                    options.split_writes,
                )?;
                for (id, method, params) in [
                    (900, "workspace/configuration", json!({"items": [{}, {}]})),
                    (901, "workspace/workspaceFolders", Value::Null),
                    (
                        902,
                        "window/showMessageRequest",
                        json!({"message": "fake server message"}),
                    ),
                    (903, "workspace/applyEdit", json!({"edit": {}})),
                    (904, "fake/unknownRequest", Value::Null),
                ] {
                    send_frame(
                        &mut output,
                        &json!({
                            "jsonrpc": "2.0", "id": id, "method": method, "params": params
                        }),
                        options.split_writes,
                    )?;
                }
            }
            (Some("shutdown"), Some(id)) => {
                send_frame(
                    &mut output,
                    &json!({"jsonrpc": "2.0", "id": id, "result": null}),
                    options.split_writes,
                )?;
                if options.late_response {
                    send_frame(
                        &mut output,
                        &json!({"jsonrpc": "2.0", "id": 777, "result": null}),
                        options.split_writes,
                    )?;
                }
            }
            (Some("textDocument/didOpen"), None) => {
                opened_document = Some(message.clone());
                if options.malformed {
                    output
                        .write_all(b"Content-Length: 1\r\n\r\n{")
                        .map_err(|error| error.to_string())?;
                    output.flush().map_err(|error| error.to_string())?;
                } else if options.oversized {
                    output
                        .write_all(b"Content-Length: 16777217\r\n\r\n")
                        .map_err(|error| error.to_string())?;
                    output.flush().map_err(|error| error.to_string())?;
                } else if options.diagnostics || options.versionless_diagnostics {
                    publish_diagnostics(
                        &mut output,
                        &message,
                        !options.versionless_diagnostics,
                        options.split_writes,
                    )?;
                }
                if options.exit_on_open {
                    std::process::exit(23);
                }
            }
            (Some("textDocument/didChange"), None) => {
                if options.stale_diagnostics {
                    publish_diagnostics(
                        &mut output,
                        opened_document.as_ref().unwrap(),
                        true,
                        options.split_writes,
                    )?;
                } else if options.diagnostics || options.versionless_diagnostics {
                    publish_diagnostics(
                        &mut output,
                        &message,
                        !options.versionless_diagnostics,
                        options.split_writes,
                    )?;
                }
                if let Some(id) = delayed_completion.take() {
                    write_completion(&mut output, id, options.split_writes)?;
                }
                if let Some(id) = delayed_hover.take() {
                    write_hover(&mut output, id, options.split_writes)?;
                }
                if let Some((id, uri)) = delayed_definition.take() {
                    write_definitions(
                        &mut output,
                        id,
                        uri,
                        options.split_writes,
                        options.one_definition,
                    )?;
                }
            }
            (Some("textDocument/completion"), Some(id)) if options.delay_features => {
                delayed_completion = Some(id);
            }
            (Some("textDocument/completion"), Some(id)) if options.feature_errors => send_frame(
                &mut output,
                &json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {"code": -32800, "message": "request cancelled"}
                }),
                options.split_writes,
            )?,
            (Some("textDocument/completion"), Some(id)) => {
                write_completion(&mut output, id, options.split_writes)?;
            }
            (Some("textDocument/hover"), Some(id)) if options.delay_features => {
                delayed_hover = Some(id);
            }
            (Some("textDocument/hover"), Some(id)) => {
                write_hover(&mut output, id, options.split_writes)?;
            }
            (Some("textDocument/definition"), Some(id)) => {
                let uri = message
                    .pointer("/params/textDocument/uri")
                    .cloned()
                    .unwrap();
                if options.delay_features {
                    delayed_definition = Some((id, uri));
                } else {
                    write_definitions(
                        &mut output,
                        id,
                        uri,
                        options.split_writes,
                        options.one_definition,
                    )?;
                }
            }
            (Some("exit"), None) => break,
            _ => {}
        }
    }
    Ok(())
}

fn parse_options() -> Result<Options, String> {
    let mut options = Options::default();
    let mut arguments = std::env::args_os().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--hang") => loop {
                std::thread::sleep(Duration::from_secs(60));
            },
            Some("--record") => {
                options.record = Some(arguments.next().ok_or("missing record path")?.into())
            }
            Some("--diagnostics") => options.diagnostics = true,
            Some("--versionless-diagnostics") => options.versionless_diagnostics = true,
            Some("--stale-diagnostics") => options.stale_diagnostics = true,
            Some("--delay-features") => options.delay_features = true,
            Some("--delay-initialize") => options.delay_initialize = true,
            Some("--feature-errors") => options.feature_errors = true,
            Some("--one-definition") => options.one_definition = true,
            Some("--full-sync") => options.full_sync = true,
            Some("--missing-sync") => options.missing_sync = true,
            Some("--split-writes") => options.split_writes = true,
            Some("--server-requests") => options.server_requests = true,
            Some("--malformed") => options.malformed = true,
            Some("--oversized") => options.oversized = true,
            Some("--exit-on-open") => options.exit_on_open = true,
            Some("--late-response") => options.late_response = true,
            Some("--stderr-lines") => {
                options.stderr_lines = arguments
                    .next()
                    .and_then(|value| value.to_str().and_then(|value| value.parse().ok()))
                    .ok_or("invalid stderr line count")?;
            }
            Some("--descendant-pid") => {
                options.descendant_pid = Some(
                    arguments
                        .next()
                        .ok_or("missing descendant PID path")?
                        .into(),
                );
            }
            _ => {}
        }
    }
    Ok(options)
}

fn record(path: &Option<PathBuf>, message: &Value) -> Result<(), String> {
    let Some(path) = path else {
        return Ok(());
    };
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("cannot open record: {error}"))?;
    serde_json::to_writer(&mut file, message).map_err(|error| error.to_string())?;
    file.write_all(b"\n").map_err(|error| error.to_string())
}

fn send_frame(output: &mut impl Write, message: &Value, split: bool) -> Result<(), String> {
    if !split {
        return editur::lsp::write_frame(output, message);
    }
    let body = serde_json::to_vec(message).map_err(|error| error.to_string())?;
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    for chunk in header.as_bytes().chunks(3).chain(body.chunks(5)) {
        output.write_all(chunk).map_err(|error| error.to_string())?;
        output.flush().map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn publish_diagnostics(
    output: &mut impl Write,
    message: &Value,
    versioned: bool,
    split: bool,
) -> Result<(), String> {
    let uri = message
        .pointer("/params/textDocument/uri")
        .cloned()
        .unwrap();
    let version = message.pointer("/params/textDocument/version").cloned();
    let mut params = json!({
        "uri": uri,
        "diagnostics": [{
            "range": {
                "start": {"line": 0, "character": 4},
                "end": {"line": 0, "character": 6}
            },
            "severity": 1,
            "source": "fake-lsp",
            "message": "fake error"
        }]
    });
    if versioned {
        params["version"] = version.unwrap_or(Value::Null);
    }
    send_frame(
        output,
        &json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": params
        }),
        split,
    )
}

fn write_completion(output: &mut impl Write, id: Value, split: bool) -> Result<(), String> {
    send_frame(
        output,
        &json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": [
                {
                    "label": "print",
                    "kind": 3,
                    "detail": "Function\ndetails",
                    "textEdit": {
                        "range": {
                            "start": {"line": 0, "character": 0},
                            "end": {"line": 0, "character": 3}
                        },
                        "newText": "println"
                    }
                },
                {"label": "snippet", "insertText": "${1:value}", "insertTextFormat": 2},
                {"label": "fallback", "insertText": "fallback"}
            ]
        }),
        split,
    )
}

fn write_hover(output: &mut impl Write, id: Value, split: bool) -> Result<(), String> {
    send_frame(
        output,
        &json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "contents": {"kind": "markdown", "value": "**fake hover**"},
                "range": {
                    "start": {"line": 0, "character": 0},
                    "end": {"line": 0, "character": 3}
                }
            }
        }),
        split,
    )
}

fn write_definitions(
    output: &mut impl Write,
    id: Value,
    uri: Value,
    split: bool,
    one: bool,
) -> Result<(), String> {
    let mut locations = vec![json!({
        "uri": uri,
        "range": {
            "start": {"line": 0, "character": 0},
            "end": {"line": 0, "character": 3}
        }
    })];
    if !one {
        locations.push(json!({
            "uri": uri,
            "range": {
                "start": {"line": 1, "character": 0},
                "end": {"line": 1, "character": 3}
            }
        }));
    }
    send_frame(
        output,
        &json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": locations
        }),
        split,
    )
}
