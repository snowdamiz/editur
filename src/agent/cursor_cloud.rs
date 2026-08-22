use std::{
    io::{Read as _, Write as _},
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use agent_client_protocol::AcpAgentConfig;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

const AGENT_URL_PREFIX: &str = "https://cursor.com/agents/";
#[cfg(feature = "network")]
const API_URL: &str = "https://api.cursor.com/v1/agents";
const MAX_OUTPUT_BYTES: usize = 256 * 1024;
const MAX_SSE_EVENT_BYTES: usize = 1024 * 1024;
const READY_TIMEOUT: Duration = Duration::from_secs(30);
const START_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct StartedAgent {
    pub id: String,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum StreamEvent {
    Assistant(String),
    Thinking(String),
    Tool {
        call_id: String,
        name: String,
        status: String,
        args: Option<serde_json::Value>,
        result: Option<serde_json::Value>,
    },
}

#[derive(Default)]
struct SseDecoder {
    buffer: Vec<u8>,
    assistant_seen: bool,
    done: bool,
}

impl SseDecoder {
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<StreamEvent>, String> {
        self.buffer.extend_from_slice(bytes);
        let mut output = Vec::new();
        while let Some((end, delimiter)) = sse_delimiter(&self.buffer) {
            if end > MAX_SSE_EVENT_BYTES {
                return Err("Cursor Cloud sent an oversized progress event".into());
            }
            let event = self.buffer.drain(..end).collect::<Vec<_>>();
            self.buffer.drain(..delimiter);
            if let Some(event) = self.parse_event(&event)? {
                output.push(event);
            }
        }
        if self.buffer.len() > MAX_SSE_EVENT_BYTES {
            return Err("Cursor Cloud sent an oversized progress event".into());
        }
        Ok(output)
    }

    fn parse_event(&mut self, bytes: &[u8]) -> Result<Option<StreamEvent>, String> {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| "Cursor Cloud sent invalid progress data".to_owned())?;
        let mut kind = "message";
        let mut data = String::new();
        for line in text.lines() {
            if let Some(value) = line.strip_prefix("event:") {
                kind = value.trim();
            } else if let Some(value) = line.strip_prefix("data:") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(value.strip_prefix(' ').unwrap_or(value));
            }
        }
        if matches!(kind, "heartbeat" | "status" | "interaction_update") {
            return Ok(None);
        }
        if kind == "done" {
            self.done = true;
            return Ok(None);
        }
        if data.is_empty() {
            return Ok(None);
        }
        let value: serde_json::Value = serde_json::from_str(&data)
            .map_err(|_| "Cursor Cloud sent invalid progress data".to_owned())?;
        let string = |field: &str, limit: usize| {
            value
                .get(field)
                .and_then(serde_json::Value::as_str)
                .filter(|value| value.len() <= limit)
                .map(str::to_owned)
        };
        match kind {
            "assistant" => {
                let text = string("text", MAX_SSE_EVENT_BYTES)
                    .ok_or_else(|| "Cursor Cloud sent invalid assistant progress".to_owned())?;
                self.assistant_seen = true;
                Ok(Some(StreamEvent::Assistant(text)))
            }
            "thinking" => Ok(Some(StreamEvent::Thinking(
                string("text", MAX_SSE_EVENT_BYTES)
                    .ok_or_else(|| "Cursor Cloud sent invalid thinking progress".to_owned())?,
            ))),
            "tool_call" => Ok(Some(StreamEvent::Tool {
                call_id: string("callId", 256)
                    .ok_or_else(|| "Cursor Cloud sent an invalid tool call".to_owned())?,
                name: string("name", 256)
                    .ok_or_else(|| "Cursor Cloud sent an invalid tool call".to_owned())?,
                status: string("status", 32)
                    .ok_or_else(|| "Cursor Cloud sent an invalid tool call".to_owned())?,
                args: value.get("args").cloned(),
                result: value.get("result").cloned(),
            })),
            "result" => {
                let status = string("status", 32)
                    .ok_or_else(|| "Cursor Cloud sent an invalid run result".to_owned())?;
                match status.as_str() {
                    "FINISHED" => {}
                    "CANCELLED" => return Err("Cursor Cloud run was cancelled".into()),
                    "EXPIRED" => return Err("Cursor Cloud run expired".into()),
                    "ERROR" => return Err("Cursor Cloud run failed".into()),
                    _ => return Err("Cursor Cloud sent an invalid run status".into()),
                }
                Ok((!self.assistant_seen)
                    .then(|| string("text", MAX_SSE_EVENT_BYTES))
                    .flatten()
                    .map(StreamEvent::Assistant))
            }
            "error" => Err("Cursor Cloud stream failed".into()),
            _ => Ok(None),
        }
    }
}

fn sse_delimiter(bytes: &[u8]) -> Option<(usize, usize)> {
    let lf = bytes.windows(2).position(|window| window == b"\n\n");
    let crlf = bytes.windows(4).position(|window| window == b"\r\n\r\n");
    match (lf, crlf) {
        (Some(left), Some(right)) if left <= right => Some((left, 2)),
        (Some(left), Some(_)) => Some((left, 2)),
        (None, Some(right)) => Some((right, 4)),
        (Some(left), None) => Some((left, 2)),
        (None, None) => None,
    }
}

fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn started_from_output(output: &str) -> Option<StartedAgent> {
    let message = output.rfind("Open Cloud Agent in:")?;
    let start = output[message..].find(AGENT_URL_PREFIX)? + message;
    let id = output[start + AGENT_URL_PREFIX.len()..]
        .split(|character: char| {
            !character.is_ascii_alphanumeric() && !matches!(character, '-' | '_')
        })
        .next()?;
    safe_id(id).then(|| StartedAgent {
        id: id.to_owned(),
        url: format!("{AGENT_URL_PREFIX}{id}"),
    })
}

fn git(project_root: &Path, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(args)
        .output()
        .map_err(|error| format!("cannot run git: {error}"))?;
    if !output.status.success() {
        return Err("the current Git branch has no pushed upstream".into());
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_owned())
        .map_err(|_| "git returned invalid text".to_owned())
}

fn validate_project(project_root: &Path) -> Result<(), String> {
    if !git(project_root, &["status", "--porcelain"])?.is_empty()
        || git(project_root, &["rev-parse", "HEAD"])?
            != git(project_root, &["rev-parse", "@{upstream}"])?
    {
        return Err("commit and push changes before starting Cursor Cloud".into());
    }
    Ok(())
}

fn cloud_command(agent: &AcpAgentConfig, project_root: &Path) -> CommandBuilder {
    let mut command = CommandBuilder::new(agent.command());
    command.args(agent.arguments());
    for (name, value) in agent.environment() {
        command.env(name, value);
    }
    command.cwd(project_root);
    command
}

fn visible_error(screen: &str) -> String {
    let detail = screen
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join(" ");
    if detail.is_empty() {
        "Cursor CLI exited before creating a Cloud Agent".into()
    } else {
        format!("Cursor CLI did not create a Cloud Agent: {detail}")
    }
}

pub(super) fn start(
    agent: &AcpAgentConfig,
    project_root: &Path,
    prompt: &str,
    shutdown: &AtomicBool,
) -> Result<StartedAgent, String> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err("Cursor Cloud prompt cannot be empty".into());
    }
    if prompt.len() > 256 * 1024
        || prompt
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
    {
        return Err("Cursor Cloud prompt contains unsupported input".into());
    }
    validate_project(project_root)?;

    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 40,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| format!("cannot open Cursor terminal: {error}"))?;
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| format!("cannot read Cursor terminal: {error}"))?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|error| format!("cannot write Cursor terminal: {error}"))?;
    let mut child = pair
        .slave
        .spawn_command(cloud_command(agent, project_root))
        .map_err(|error| format!("cannot start Cursor CLI: {error}"))?;
    drop(pair.slave);

    let (send_output, output) = mpsc::sync_channel(64);
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 8 * 1024];
        while let Ok(read) = reader.read(&mut buffer) {
            if read == 0 || send_output.send(buffer[..read].to_vec()).is_err() {
                break;
            }
        }
    });

    let started_at = Instant::now();
    let mut parser = vt100::Parser::new(40, 120, 100);
    let mut raw_output = Vec::new();
    let mut cloud_requested_at = None;
    let mut prompt_sent = false;
    loop {
        if shutdown.load(Ordering::Acquire) {
            let _ = child.kill();
            return Err("Cursor Cloud start was cancelled".into());
        }
        if started_at.elapsed() >= START_TIMEOUT {
            let _ = child.kill();
            return Err("Cursor CLI timed out while starting a Cloud Agent".into());
        }
        if let Ok(bytes) = output.recv_timeout(Duration::from_millis(100)) {
            parser.process(&bytes);
            raw_output.extend_from_slice(&bytes);
            if raw_output.len() > MAX_OUTPUT_BYTES {
                raw_output.drain(..raw_output.len() - MAX_OUTPUT_BYTES);
            }
        }
        let screen = parser.screen().contents();
        if screen.contains("Set up cloud agents") || screen.contains("Please set up cloud agents") {
            let _ = child.kill();
            return Err(
                "set up Cursor Cloud Agents for this account: https://cursor.com/dashboard?tab=cloud-agents"
                    .into(),
            );
        }

        if cloud_requested_at.is_none()
            && (screen.contains("to move to cloud")
                || screen.contains("Plan, search, build anything")
                || screen.contains("Add a follow-up"))
        {
            let result = writer.write_all(b"&").and_then(|()| writer.flush());
            if let Err(error) = result {
                let _ = child.kill();
                return Err(format!("cannot select Cursor Cloud mode: {error}"));
            }
            cloud_requested_at = Some(Instant::now());
        } else if !prompt_sent
            && cloud_requested_at.is_some()
            && (screen.contains("Move to cloud agent") || screen.contains("^ Cloud agent"))
        {
            let result = writer
                .write_all(b"\x1b[200~")
                .and_then(|()| writer.write_all(prompt.as_bytes()))
                .and_then(|()| writer.write_all(b"\x1b[201~"))
                .and_then(|()| writer.write_all(b"\r"))
                .and_then(|()| writer.flush());
            if let Err(error) = result {
                let _ = child.kill();
                return Err(format!("cannot submit Cursor Cloud prompt: {error}"));
            }
            prompt_sent = true;
        }

        if cloud_requested_at.is_none() && started_at.elapsed() >= READY_TIMEOUT {
            let _ = child.kill();
            return Err("Cursor CLI did not become ready for Cloud handoff".into());
        }
        if cloud_requested_at.is_some_and(|started| started.elapsed() >= READY_TIMEOUT)
            && !prompt_sent
        {
            let _ = child.kill();
            return Err(
                "this Cursor CLI does not offer Cloud handoff for the selected account".into(),
            );
        }
        let status = match child.try_wait() {
            Ok(status) => status,
            Err(error) => {
                let _ = child.kill();
                return Err(format!("cannot wait for Cursor CLI: {error}"));
            }
        };
        if let Some(status) = status {
            while let Ok(bytes) = output.recv_timeout(Duration::from_millis(50)) {
                parser.process(&bytes);
                raw_output.extend_from_slice(&bytes);
                if raw_output.len() > MAX_OUTPUT_BYTES {
                    raw_output.drain(..raw_output.len() - MAX_OUTPUT_BYTES);
                }
            }
            if status.success()
                && let Some(started) = started_from_output(&String::from_utf8_lossy(&raw_output))
            {
                return Ok(started);
            }
            let screen = parser.screen().contents();
            return Err(visible_error(&screen));
        }
    }
}

#[cfg(feature = "network")]
pub(super) fn stream(
    started: &StartedAgent,
    api_key: &str,
    shutdown: &AtomicBool,
    mut on_event: impl FnMut(StreamEvent),
) -> Result<(), String> {
    if !safe_id(&started.id) {
        return Err("Cursor Cloud returned an invalid agent ID".into());
    }
    let client: ureq::Agent = ureq::Agent::config_builder()
        .user_agent(format!("editur/{}", env!("CARGO_PKG_VERSION")))
        .timeout_connect(Some(Duration::from_secs(15)))
        .http_status_as_error(false)
        .build()
        .into();
    let authorization = format!("Bearer {api_key}");
    let mut agent = client
        .get(format!("{API_URL}/{}", started.id))
        .header("Authorization", &authorization)
        .header("Accept", "application/json")
        .call()
        .map_err(|_| "cannot connect to the Cursor Cloud progress API".to_owned())?;
    cursor_api_status(agent.status().as_u16())?;
    let bytes = agent
        .body_mut()
        .with_config()
        .limit((MAX_OUTPUT_BYTES + 1) as u64)
        .read_to_vec()
        .map_err(|_| "cannot read the Cursor Cloud agent record".to_owned())?;
    if bytes.len() > MAX_OUTPUT_BYTES {
        return Err("Cursor Cloud returned an oversized agent record".into());
    }
    let run_id = serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|value| value.get("latestRunId")?.as_str().map(str::to_owned))
        .filter(|run_id| safe_id(run_id))
        .ok_or_else(|| "Cursor Cloud did not return a valid run ID".to_owned())?;

    let mut response = client
        .get(format!("{API_URL}/{}/runs/{run_id}/stream", started.id))
        .header("Authorization", &authorization)
        .header("Accept", "text/event-stream")
        .call()
        .map_err(|_| "cannot connect to the Cursor Cloud progress stream".to_owned())?;
    cursor_api_status(response.status().as_u16())?;
    let mut reader = response.body_mut().as_reader();
    let mut decoder = SseDecoder::default();
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        if shutdown.load(Ordering::Acquire) {
            return Ok(());
        }
        let count = reader
            .read(&mut buffer)
            .map_err(|_| "Cursor Cloud progress stream disconnected".to_owned())?;
        if count == 0 {
            return if decoder.done {
                Ok(())
            } else {
                Err("Cursor Cloud progress stream ended early".into())
            };
        }
        for event in decoder.push(&buffer[..count])? {
            on_event(event);
        }
        if decoder.done {
            return Ok(());
        }
    }
}

#[cfg(feature = "network")]
fn cursor_api_status(status: u16) -> Result<(), String> {
    match status {
        200..=299 => Ok(()),
        401 | 403 => Err("Cursor Cloud API key was rejected".into()),
        404 => Err("Cursor Cloud agent is not available to this API key".into()),
        429 => Err("Cursor Cloud progress API is rate limited".into()),
        _ => Err(format!("Cursor Cloud progress API returned HTTP {status}")),
    }
}

#[cfg(not(feature = "network"))]
pub(super) fn stream(
    _started: &StartedAgent,
    _api_key: &str,
    _shutdown: &AtomicBool,
    _on_event: impl FnMut(StreamEvent),
) -> Result<(), String> {
    Err("live Cursor Cloud progress is unavailable in this build".into())
}

#[cfg(test)]
mod tests {
    use std::{fs, process::Command};

    fn git(project: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(project)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    fn project() -> tempfile::TempDir {
        let project = tempfile::tempdir().unwrap();
        git(
            project.path(),
            &["init", "--quiet", "--initial-branch=main"],
        );
        git(
            project.path(),
            &["config", "user.email", "test@example.com"],
        );
        git(project.path(), &["config", "user.name", "Test"]);
        fs::write(project.path().join("README.md"), "cloud fixture\n").unwrap();
        git(project.path(), &["add", "README.md"]);
        git(project.path(), &["commit", "--quiet", "-m", "fixture"]);
        git(
            project.path(),
            &["remote", "add", "origin", "https://example.com/repo.git"],
        );
        git(
            project.path(),
            &["update-ref", "refs/remotes/origin/main", "HEAD"],
        );
        git(project.path(), &["config", "branch.main.remote", "origin"]);
        git(
            project.path(),
            &["config", "branch.main.merge", "refs/heads/main"],
        );
        project
    }

    #[test]
    fn start_rejects_changes_the_cloud_cannot_see() {
        let project = project();
        fs::write(project.path().join("README.md"), "local-only change\n").unwrap();

        assert_eq!(
            super::validate_project(project.path()).unwrap_err(),
            "commit and push changes before starting Cursor Cloud"
        );
    }

    #[test]
    fn parses_the_agent_created_by_cursor_cli() {
        let started = super::started_from_output(
            "\u{1b}[90mOpen Cloud Agent in:\u{1b}[0m  \u{1b}[36mhttps://cursor.com/agents/bc-agent_1\u{1b}[0m",
        )
        .unwrap();

        assert_eq!(
            started,
            super::StartedAgent {
                id: "bc-agent_1".into(),
                url: "https://cursor.com/agents/bc-agent_1".into(),
            }
        );
    }

    #[test]
    fn cloud_handoff_reuses_the_selected_account_launcher() {
        let config = agent_client_protocol::AcpAgentConfig::new("/editur").args([
            "--agent-process",
            "cursor",
            "7",
            "/project",
        ]);
        let command = super::cloud_command(&config, std::path::Path::new("/project"));

        assert_eq!(
            command.get_argv(),
            &["/editur", "--agent-process", "cursor", "7", "/project"]
                .map(std::ffi::OsString::from)
                .to_vec()
        );
    }

    #[test]
    fn parses_fragmented_cursor_cloud_progress_without_repeating_the_result() {
        let mut decoder = super::SseDecoder::default();
        let mut events = decoder
            .push(b"event: thinking\ndata: {\"text\":\"Checking the project\"}\n\nevent: ass")
            .unwrap();
        events.extend(
            decoder
                .push(
                    b"istant\ndata: {\"text\":\"I found it.\"}\n\nevent: tool_call\ndata: {\"callId\":\"call-1\",\"name\":\"Read\",\"status\":\"completed\",\"args\":{\"path\":\"README.md\"}}\n\nevent: result\ndata: {\"status\":\"FINISHED\",\"text\":\"I found it.\"}\n\nevent: done\ndata: {}\n\n",
                )
                .unwrap(),
        );

        assert_eq!(
            events,
            vec![
                super::StreamEvent::Thinking("Checking the project".into()),
                super::StreamEvent::Assistant("I found it.".into()),
                super::StreamEvent::Tool {
                    call_id: "call-1".into(),
                    name: "Read".into(),
                    status: "completed".into(),
                    args: Some(serde_json::json!({"path": "README.md"})),
                    result: None,
                },
            ]
        );
    }
}
