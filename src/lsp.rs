use std::{
    ffi::OsStr,
    io::{BufRead, Read as _},
    ops::Range as ByteRange,
    path::{Path, PathBuf},
    str::FromStr,
};

use serde::Deserialize;
use serde_json::Value;

mod controller;
pub use controller::*;

const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

pub fn read_frame(reader: &mut impl BufRead) -> Result<Vec<u8>, String> {
    let mut header_bytes = 0;
    let mut content_length = None;
    loop {
        let mut line = Vec::new();
        let remaining = MAX_HEADER_BYTES.saturating_sub(header_bytes) + 1;
        let read = (&mut *reader)
            .take(remaining as u64)
            .read_until(b'\n', &mut line)
            .map_err(|error| format!("cannot read LSP header: {error}"))?;
        if read == 0 {
            return Err("premature EOF while reading LSP headers".into());
        }
        header_bytes += read;
        if header_bytes > MAX_HEADER_BYTES {
            return Err("LSP header exceeds 16 KiB".into());
        }
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if line.is_empty() {
            break;
        }
        let line = std::str::from_utf8(&line).map_err(|_| "LSP header is not UTF-8")?;
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| "malformed LSP header".to_owned())?;
        if !name.trim().eq_ignore_ascii_case("Content-Length") {
            continue;
        }
        if content_length.is_some() {
            return Err("duplicate LSP Content-Length header".into());
        }
        let value = value.trim();
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("invalid LSP Content-Length header".into());
        }
        let length = value
            .parse::<usize>()
            .map_err(|_| "invalid LSP Content-Length header")?;
        if length > MAX_BODY_BYTES {
            return Err("LSP body exceeds 16 MiB".into());
        }
        content_length = Some(length);
    }
    let length = content_length.ok_or_else(|| "missing LSP Content-Length header".to_owned())?;
    let mut body = vec![0; length];
    reader
        .read_exact(&mut body)
        .map_err(|error| format!("premature EOF while reading LSP body: {error}"))?;
    Ok(body)
}

pub fn write_frame(writer: &mut impl std::io::Write, message: &Value) -> Result<(), String> {
    let body =
        serde_json::to_vec(message).map_err(|error| format!("cannot encode LSP JSON: {error}"))?;
    if body.len() > MAX_BODY_BYTES {
        return Err("LSP body exceeds 16 MiB".into());
    }
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())
        .and_then(|()| writer.write_all(&body))
        .and_then(|()| writer.flush())
        .map_err(|error| format!("cannot write LSP frame: {error}"))
}

#[derive(Debug)]
enum WireMessage {
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    Response {
        id: u64,
        result: Result<Value, RpcError>,
    },
}

#[derive(Debug, Deserialize)]
struct RpcError {
    code: i64,
    message: String,
    #[serde(default)]
    data: Option<Value>,
}

fn decode_message(body: &[u8]) -> Result<WireMessage, String> {
    let value: Value =
        serde_json::from_slice(body).map_err(|error| format!("invalid LSP JSON: {error}"))?;
    let Value::Object(mut object) = value else {
        return Err("LSP message is not an object".to_owned());
    };
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err("LSP message is not JSON-RPC 2.0".into());
    }
    if let Some(Value::String(method)) = object.remove("method") {
        if method.len() > 1_024 {
            return Err("LSP method exceeds 1 KiB".into());
        }
        if object.contains_key("result") || object.contains_key("error") {
            return Err("LSP method message contains a response payload".into());
        }
        let params = object.remove("params").unwrap_or(Value::Null);
        if !params.is_null() && !params.is_object() && !params.is_array() {
            return Err("LSP params must be an object or array".into());
        }
        return match object.remove("id") {
            None | Some(Value::Null) => Ok(WireMessage::Notification { method, params }),
            Some(id) if id.is_number() => Ok(WireMessage::Request { id, method, params }),
            Some(Value::String(id)) if id.len() <= 1_024 => Ok(WireMessage::Request {
                id: Value::String(id),
                method,
                params,
            }),
            Some(Value::String(_)) => Err("LSP request ID exceeds 1 KiB".into()),
            Some(_) => Err("invalid LSP request ID".into()),
        };
    }
    let id = object
        .remove("id")
        .and_then(|id| id.as_u64())
        .ok_or_else(|| "invalid LSP response ID".to_owned())?;
    match (object.remove("result"), object.remove("error")) {
        (Some(result), None) => Ok(WireMessage::Response {
            id,
            result: Ok(result),
        }),
        (None, Some(error)) => Ok(WireMessage::Response {
            id,
            result: Err(serde_json::from_value(error)
                .map_err(|error| format!("invalid LSP error response: {error}"))?),
        }),
        _ => Err("LSP response must contain exactly one result or error".into()),
    }
}

pub fn incremental_change(old: &str, new: &str) -> lsp_types::TextDocumentContentChangeEvent {
    let shared = old.len().min(new.len());
    let mut prefix = old
        .as_bytes()
        .iter()
        .zip(new.as_bytes())
        .take(shared)
        .take_while(|(old, new)| old == new)
        .count();
    while !old.is_char_boundary(prefix) || !new.is_char_boundary(prefix) {
        prefix -= 1;
    }

    let mut suffix = old.as_bytes()[prefix..]
        .iter()
        .rev()
        .zip(new.as_bytes()[prefix..].iter().rev())
        .take_while(|(old, new)| old == new)
        .count();
    while !old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix) {
        suffix -= 1;
    }
    let old_end = old.len() - suffix;
    let new_end = new.len() - suffix;
    lsp_types::TextDocumentContentChangeEvent {
        range: Some(lsp_types::Range::new(
            position_for_byte(old, prefix),
            position_for_byte(old, old_end),
        )),
        range_length: None,
        text: new[prefix..new_end].to_owned(),
    }
}

pub fn position_for_byte(text: &str, byte: usize) -> lsp_types::Position {
    let mut byte = byte.min(text.len());
    while !text.is_char_boundary(byte) {
        byte -= 1;
    }
    let prefix = &text[..byte];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() as u32;
    let line_start = prefix.rfind('\n').map_or(0, |index| index + 1);
    let character = prefix[line_start..].encode_utf16().count() as u32;
    lsp_types::Position::new(line, character)
}

pub fn byte_for_position(text: &str, position: lsp_types::Position) -> usize {
    let mut line_start = 0;
    for _ in 0..position.line {
        let Some(newline) = text[line_start..].find('\n') else {
            return text.len();
        };
        line_start += newline + 1;
    }
    let line_end = text[line_start..]
        .find('\n')
        .map_or(text.len(), |newline| line_start + newline);
    let mut utf16 = 0;
    for (byte, character) in text[line_start..line_end].char_indices() {
        let next = utf16 + character.len_utf16() as u32;
        if next > position.character {
            return line_start + byte;
        }
        utf16 = next;
        if utf16 == position.character {
            return line_start + byte + character.len_utf8();
        }
    }
    line_end
}

pub fn byte_range_for_lsp_range(text: &str, range: lsp_types::Range) -> Option<ByteRange<usize>> {
    let start = byte_for_position(text, range.start);
    let end = byte_for_position(text, range.end);
    (start <= end).then_some(start..end)
}

pub fn discover_executable(command: &str, path: Option<&OsStr>) -> Result<Option<PathBuf>, String> {
    let command_path = Path::new(command);
    if command_path.is_absolute() {
        return match std::fs::metadata(command_path) {
            Ok(metadata) if metadata.is_file() => Ok(Some(command_path.to_path_buf())),
            Ok(_) => Err(format!("{} is not a regular file", command_path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!(
                "cannot inspect {}: {error}",
                command_path.display()
            )),
        };
    }
    if command_path.components().count() != 1 {
        return Err(format!(
            "{command} must be an absolute path or bare executable name"
        ));
    }
    let inherited_path;
    let path = if let Some(path) = path {
        path
    } else {
        inherited_path = std::env::var_os("PATH");
        let Some(path) = inherited_path.as_deref() else {
            return Ok(None);
        };
        path
    };
    for directory in std::env::split_paths(path) {
        for candidate in executable_candidates(&directory, command) {
            let candidate = if candidate.is_absolute() {
                candidate
            } else {
                std::env::current_dir()
                    .map_err(|error| format!("cannot resolve executable search path: {error}"))?
                    .join(candidate)
            };
            match std::fs::metadata(&candidate) {
                Ok(metadata) if metadata.is_file() => {
                    if command == "rust-analyzer" {
                        let rustup = candidate.with_file_name(if cfg!(windows) {
                            "rustup.exe"
                        } else {
                            "rustup"
                        });
                        let is_rustup_proxy = matches!(
                            (
                                std::fs::canonicalize(&candidate),
                                std::fs::canonicalize(&rustup)
                            ),
                            (Ok(candidate), Ok(rustup)) if candidate == rustup
                        );
                        if is_rustup_proxy
                            && !std::process::Command::new(&rustup)
                                .args(["which", command])
                                .stdout(std::process::Stdio::null())
                                .stderr(std::process::Stdio::null())
                                .status()
                                .map_err(|error| {
                                    format!("cannot query {}: {error}", rustup.display())
                                })?
                                .success()
                        {
                            continue;
                        }
                    }
                    return Ok(Some(candidate));
                }
                Ok(_) => continue,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(format!("cannot inspect {}: {error}", candidate.display()));
                }
            }
        }
    }
    Ok(None)
}

fn executable_candidates(directory: &Path, command: &str) -> Vec<PathBuf> {
    let exact = directory.join(command);
    #[cfg(windows)]
    {
        if exact.extension().is_none() {
            return vec![exact.clone(), exact.with_extension("exe")];
        }
    }
    vec![exact]
}

pub fn file_uri(path: &Path) -> Result<lsp_types::Uri, String> {
    if !path.is_absolute() {
        return Err(format!("{} is not an absolute path", path.display()));
    }
    let path = path
        .to_str()
        .ok_or_else(|| format!("{} is not a Unicode path", path.display()))?;
    #[cfg(windows)]
    let path = path.replace('\\', "/");
    let mut encoded = String::with_capacity(path.len() + 7);
    encoded.push_str("file://");
    #[cfg(windows)]
    if !path.starts_with('/') {
        encoded.push('/');
    }
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/' | b':') {
            encoded.push(byte as char);
        } else {
            use std::fmt::Write as _;
            write!(encoded, "%{byte:02X}").expect("writing to a String cannot fail");
        }
    }
    lsp_types::Uri::from_str(&encoded)
        .map_err(|error| format!("cannot create URI for {}: {error}", path))
}

pub fn path_from_file_uri(uri: &lsp_types::Uri) -> Result<PathBuf, String> {
    let value = uri.as_str();
    let path = value
        .strip_prefix("file://")
        .filter(|path| path.starts_with('/'))
        .ok_or_else(|| "definition URI is not a local file URI".to_owned())?;
    if path.contains(['?', '#']) {
        return Err("definition file URI contains a query or fragment".into());
    }
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err("definition file URI has invalid percent encoding".into());
            }
            let high = hex(bytes[index + 1])
                .ok_or_else(|| "definition file URI has invalid percent encoding".to_owned())?;
            let low = hex(bytes[index + 2])
                .ok_or_else(|| "definition file URI has invalid percent encoding".to_owned())?;
            decoded.push(high * 16 + low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    let path =
        String::from_utf8(decoded).map_err(|_| "definition file URI is not UTF-8".to_owned())?;
    #[cfg(windows)]
    let path = path
        .strip_prefix('/')
        .filter(|path| path.as_bytes().get(1) == Some(&b':'))
        .unwrap_or(&path);
    Ok(PathBuf::from(path))
}

const fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn initialize_params(project_root: &Path) -> Result<Value, String> {
    let uri = file_uri(project_root)?;
    let name = project_root
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("project");
    Ok(serde_json::json!({
        "processId": std::process::id(),
        "rootUri": uri,
        "capabilities": {
            "workspace": { "workspaceFolders": true },
            "textDocument": {
                "synchronization": { "didSave": true },
                "publishDiagnostics": { "versionSupport": true },
                "completion": {
                    "completionItem": {
                        "snippetSupport": false,
                        "documentationFormat": ["markdown", "plaintext"]
                    }
                },
                "hover": { "contentFormat": ["markdown", "plaintext"] },
                "definition": { "linkSupport": true }
            }
        },
        "workspaceFolders": [{ "uri": uri, "name": name }],
        "clientInfo": { "name": "Editur", "version": env!("CARGO_PKG_VERSION") }
    }))
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PresetId {
    RustAnalyzer,
    TypeScript,
    Pyright,
    Gopls,
    Clangd,
}

impl PresetId {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RustAnalyzer => "rust-analyzer",
            Self::TypeScript => "typescript-language-server",
            Self::Pyright => "pyright",
            Self::Gopls => "gopls",
            Self::Clangd => "clangd",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Preset {
    pub id: PresetId,
    pub language: &'static str,
    pub name: &'static str,
    pub command: &'static str,
    pub args: &'static [&'static str],
}

const PRESETS: [Preset; 5] = [
    Preset {
        id: PresetId::RustAnalyzer,
        language: "Rust",
        name: "rust-analyzer",
        command: "rust-analyzer",
        args: &[],
    },
    Preset {
        id: PresetId::TypeScript,
        language: "TypeScript / JavaScript",
        name: "TypeScript Language Server",
        command: "typescript-language-server",
        args: &["--stdio"],
    },
    Preset {
        id: PresetId::Pyright,
        language: "Python",
        name: "Pyright",
        command: "pyright-langserver",
        args: &["--stdio"],
    },
    Preset {
        id: PresetId::Gopls,
        language: "Go",
        name: "gopls",
        command: "gopls",
        args: &[],
    },
    Preset {
        id: PresetId::Clangd,
        language: "C / C++",
        name: "clangd",
        command: "clangd",
        args: &[],
    },
];

pub const fn catalog() -> &'static [Preset] {
    &PRESETS
}

pub fn preset_for_path(path: &Path) -> Option<(&'static Preset, &'static str)> {
    let extension = path.extension()?.to_str()?;
    let (id, language) = match extension {
        "rs" => (PresetId::RustAnalyzer, "rust"),
        "ts" => (PresetId::TypeScript, "typescript"),
        "tsx" => (PresetId::TypeScript, "typescriptreact"),
        "js" => (PresetId::TypeScript, "javascript"),
        "jsx" => (PresetId::TypeScript, "javascriptreact"),
        "py" => (PresetId::Pyright, "python"),
        "go" => (PresetId::Gopls, "go"),
        "c" | "h" => (PresetId::Clangd, "c"),
        "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" => (PresetId::Clangd, "cpp"),
        _ => return None,
    };
    PRESETS
        .iter()
        .find(|preset| preset.id == id)
        .map(|preset| (preset, language))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::BufReader, path::Path};

    #[test]
    fn preset_catalog_maps_every_supported_language_to_its_shared_server() {
        let cases = [
            ("main.rs", PresetId::RustAnalyzer, "rust"),
            ("main.ts", PresetId::TypeScript, "typescript"),
            ("main.tsx", PresetId::TypeScript, "typescriptreact"),
            ("main.js", PresetId::TypeScript, "javascript"),
            ("main.jsx", PresetId::TypeScript, "javascriptreact"),
            ("main.py", PresetId::Pyright, "python"),
            ("main.go", PresetId::Gopls, "go"),
            ("main.c", PresetId::Clangd, "c"),
            ("main.cpp", PresetId::Clangd, "cpp"),
        ];

        for (path, expected_preset, expected_language) in cases {
            let (preset, language) = preset_for_path(Path::new(path)).unwrap();
            assert_eq!((preset.id, language), (expected_preset, expected_language));
        }
        assert!(preset_for_path(Path::new("README.md")).is_none());
    }

    #[test]
    fn partial_and_back_to_back_frames_parse_without_losing_bytes() {
        let bytes = b"Content-Length: 7\r\n\r\n{\"a\":1}content-length: 7\r\n\r\n{\"b\":2}";
        let mut reader = BufReader::with_capacity(2, &bytes[..]);

        assert_eq!(read_frame(&mut reader).unwrap(), br#"{"a":1}"#);
        assert_eq!(read_frame(&mut reader).unwrap(), br#"{"b":2}"#);
    }

    #[test]
    fn malformed_bounded_or_truncated_frames_fail_clearly() {
        for bytes in [
            b"X: y\r\n\r\n".as_slice(),
            b"Content-Length: nope\r\n\r\n".as_slice(),
            b"Content-Length: 1\r\nContent-Length: 1\r\n\r\nx".as_slice(),
            b"Content-Length: 5\r\n\r\nxx".as_slice(),
            b"Content-Length: 16777217\r\n\r\n".as_slice(),
        ] {
            assert!(read_frame(&mut BufReader::new(bytes)).is_err());
        }
        let oversized_header = format!("Ignored: {}\r\n\r\n", "x".repeat(16 * 1024));
        assert!(read_frame(&mut BufReader::new(oversized_header.as_bytes())).is_err());
    }

    #[test]
    fn json_rpc_envelopes_distinguish_requests_notifications_and_responses() {
        assert!(matches!(
            decode_message(br#"{"jsonrpc":"2.0","id":7,"result":null}"#).unwrap(),
            WireMessage::Response { id: 7, .. }
        ));
        assert!(matches!(
            decode_message(br#"{"jsonrpc":"2.0","method":"window/logMessage","params":{}}"#)
                .unwrap(),
            WireMessage::Notification { .. }
        ));
        assert!(matches!(
            decode_message(br#"{"jsonrpc":"2.0","id":"server-1","method":"custom/request"}"#)
                .unwrap(),
            WireMessage::Request { .. }
        ));
        for invalid in [
            br#"{"#.as_slice(),
            br#"{"jsonrpc":"1.0","method":"x"}"#,
            br#"{"jsonrpc":"2.0","id":1}"#,
            br#"{"jsonrpc":"2.0","id":1,"result":null,"error":{"code":-1,"message":"x"}}"#,
        ] {
            assert!(decode_message(invalid).is_err());
        }
        let long = "x".repeat(1_025);
        assert!(
            decode_message(
                &serde_json::to_vec(&serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": long,
                    "params": {}
                }))
                .unwrap()
            )
            .is_err()
        );
        assert!(
            decode_message(
                &serde_json::to_vec(&serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": "x".repeat(1_025),
                    "method": "bounded"
                }))
                .unwrap()
            )
            .is_err()
        );
    }

    #[test]
    fn one_incremental_replacement_handles_newlines_and_non_bmp_text() {
        let change = incremental_change("a💡b\nold", "a💡x\nnew");

        assert_eq!(change.range.unwrap().start, lsp_types::Position::new(0, 3));
        assert_eq!(change.range.unwrap().end, lsp_types::Position::new(1, 3));
        assert_eq!(change.text, "x\nnew");
        assert_eq!(
            byte_range_for_lsp_range(
                "a💡b\nold",
                lsp_types::Range::new(
                    lsp_types::Position::new(0, 1),
                    lsp_types::Position::new(0, 3),
                ),
            ),
            Some(1..5)
        );
        for (old, new) in [
            ("abc", "abxc"),
            ("abc", "ac"),
            ("abc", "axyc"),
            ("a\nb", "a\n\nb"),
            ("💡", "🦀"),
            ("paste", "a long pasted value"),
            ("undo target", "target"),
        ] {
            let change = incremental_change(old, new);
            let range = change.range.unwrap();
            let bytes = byte_range_for_lsp_range(old, range).unwrap();
            assert_eq!(
                format!(
                    "{}{}{}",
                    &old[..bytes.start],
                    change.text,
                    &old[bytes.end..]
                ),
                new
            );
        }
    }

    #[test]
    fn executable_discovery_uses_path_order_and_rejects_directories() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        std::fs::create_dir_all(first.join("server")).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(second.join("server"), b"fake").unwrap();
        let path = std::env::join_paths([&first, &second]).unwrap();

        assert_eq!(
            discover_executable("server", Some(&path)).unwrap(),
            Some(second.join("server"))
        );
        assert_eq!(
            discover_executable(first.join("server").to_str().unwrap(), Some(&path)).unwrap_err(),
            format!("{} is not a regular file", first.join("server").display())
        );
        assert_eq!(
            discover_executable(
                directory.path().join("missing-server").to_str().unwrap(),
                Some(&path),
            )
            .unwrap(),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn rustup_proxy_without_an_installed_rust_analyzer_is_skipped() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let directory = tempfile::tempdir().unwrap();
        let rustup = directory.path().join("rustup");
        std::fs::write(&rustup, "#!/bin/sh\nexit 1\n").unwrap();
        let mut permissions = std::fs::metadata(&rustup).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&rustup, permissions).unwrap();
        symlink("rustup", directory.path().join("rust-analyzer")).unwrap();

        assert_eq!(
            discover_executable("rust-analyzer", Some(directory.path().as_os_str())).unwrap(),
            None
        );
    }

    #[test]
    fn initialize_advertises_only_the_supported_client_surface() {
        let root = std::env::current_dir().unwrap();
        let params = initialize_params(&root).unwrap();

        assert_eq!(
            params.pointer("/capabilities/workspace/workspaceFolders"),
            Some(&Value::Bool(true))
        );
        assert_eq!(
            params.pointer("/capabilities/textDocument/publishDiagnostics/versionSupport"),
            Some(&Value::Bool(true))
        );
        assert_eq!(
            params.pointer("/capabilities/textDocument/definition/linkSupport"),
            Some(&Value::Bool(true))
        );
        assert_eq!(
            params.pointer("/capabilities/textDocument/completion/completionItem/snippetSupport"),
            Some(&Value::Bool(false))
        );
        assert!(
            params
                .pointer("/capabilities/textDocument/semanticTokens")
                .is_none()
        );
        assert!(
            params
                .pointer("/capabilities/workspace/applyEdit")
                .is_none()
        );
        assert!(serde_json::from_value::<lsp_types::InitializeParams>(params).is_ok());
    }
}
