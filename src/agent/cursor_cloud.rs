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
const MAX_OUTPUT_BYTES: usize = 256 * 1024;
const READY_TIMEOUT: Duration = Duration::from_secs(30);
const START_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct StartedAgent {
    pub id: String,
    pub url: String,
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
}
