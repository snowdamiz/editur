use std::ffi::OsString;
use std::path::PathBuf;

use crate::agent::provider::{AccountKey, ProviderId};

#[derive(Debug, Eq, PartialEq)]
pub enum Command {
    Open(Option<PathBuf>),
    Resident(PathBuf),
    QuitRunning,
    AgentProcess(AccountKey, PathBuf, Vec<OsString>),
    AgentProvision(ProviderId),
    Update,
    #[cfg(windows)]
    FinishUpdate(PathBuf),
    #[cfg(windows)]
    CleanupUpdate(PathBuf),
    Help,
    Version,
}

pub fn parse_args<I>(args: I) -> Result<Command, String>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = args.into_iter();
    let Some(first) = args.next() else {
        return Ok(Command::Open(None));
    };

    if first == "--resident" {
        let target = args
            .next()
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| "missing internal process target".to_owned())?;
        if args.next().is_some() {
            return Err("too many internal process arguments".into());
        }
        return Ok(Command::Resident(target));
    }

    if first == "--agent-process" {
        let provider = args
            .next()
            .and_then(|value| value.into_string().ok())
            .ok_or_else(|| "missing internal ACP provider id".to_owned())?
            .parse::<ProviderId>()?;
        let account_id = args
            .next()
            .and_then(|value| value.into_string().ok())
            .ok_or_else(|| "missing internal ACP account id".to_owned())?
            .parse::<u64>()
            .map_err(|_| "invalid internal ACP account id".to_owned())?;
        if account_id == 0 || account_id > crate::agent::provider::MAX_ACCOUNT_ID {
            return Err("internal ACP account id is outside the supported range".into());
        }
        let target = args
            .next()
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| "missing internal agent process target".to_owned())?;
        return Ok(Command::AgentProcess(
            AccountKey {
                provider,
                account_id,
            },
            target,
            args.collect(),
        ));
    }

    if first == "--provision-agent" {
        let provider = args
            .next()
            .map(|value| {
                value
                    .into_string()
                    .map_err(|_| "ACP provider id must be valid UTF-8".to_owned())?
                    .parse::<ProviderId>()
            })
            .transpose()?
            .unwrap_or(ProviderId::Cursor);
        if args.next().is_some() {
            return Err("too many internal provision arguments".into());
        }
        return Ok(Command::AgentProvision(provider));
    }

    if first == "--quit-running" {
        if args.next().is_some() {
            return Err("too many internal quit arguments".into());
        }
        return Ok(Command::QuitRunning);
    }

    #[cfg(windows)]
    if first == "--finish-update" || first == "--cleanup-update" {
        let path = args
            .next()
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| "missing internal update path".to_owned())?;
        if args.next().is_some() {
            return Err("too many internal update arguments".into());
        }
        return Ok(if first == "--finish-update" {
            Command::FinishUpdate(path)
        } else {
            Command::CleanupUpdate(path)
        });
    }

    let command = match first.to_str() {
        Some("--help" | "-h") => Command::Help,
        Some("--version" | "-V") => Command::Version,
        Some("update") => Command::Update,
        _ => Command::Open(Some(PathBuf::from(first))),
    };

    if args.next().is_some() {
        Err("too many arguments; run `editur --help` for usage".into())
    } else {
        Ok(command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Command, String> {
        parse_args(args.iter().map(OsString::from))
    }

    #[test]
    fn parses_editor_and_information_commands() {
        assert_eq!(parse(&[]), Ok(Command::Open(None)));
        assert_eq!(
            parse(&["src/main.rs"]),
            Ok(Command::Open(Some(PathBuf::from("src/main.rs"))))
        );
        assert_eq!(parse(&["--help"]), Ok(Command::Help));
        assert_eq!(parse(&["--version"]), Ok(Command::Version));
        assert_eq!(parse(&["update"]), Ok(Command::Update));
    }

    #[test]
    fn parses_hidden_resident_command_with_its_target() {
        assert_eq!(
            parse(&["--resident", "/tmp/project"]),
            Ok(Command::Resident(PathBuf::from("/tmp/project")))
        );
        assert!(parse(&["--resident"]).is_err());
        assert_eq!(parse(&["--quit-running"]), Ok(Command::QuitRunning));
    }

    #[test]
    fn parses_hidden_managed_agent_launcher_with_its_project_root() {
        assert_eq!(
            parse(&["--agent-process", "codex", "7", "/tmp/project"]),
            Ok(Command::AgentProcess(
                AccountKey {
                    provider: ProviderId::Codex,
                    account_id: 7,
                },
                PathBuf::from("/tmp/project"),
                Vec::new(),
            ))
        );
        assert_eq!(
            parse(&[
                "--agent-process",
                "claude",
                "9",
                "/tmp/project",
                "--cli",
                "auth",
                "login",
                "--claudeai",
            ]),
            Ok(Command::AgentProcess(
                AccountKey {
                    provider: ProviderId::Claude,
                    account_id: 9,
                },
                PathBuf::from("/tmp/project"),
                ["--cli", "auth", "login", "--claudeai"]
                    .into_iter()
                    .map(OsString::from)
                    .collect(),
            ))
        );
        assert!(parse(&["--agent-process"]).is_err());
        assert!(parse(&["--agent-process", "codex", "0", "/tmp/project"]).is_err());
        assert!(parse(&["--agent-process", "codex", "nope", "/tmp/project"]).is_err());
        assert_eq!(
            parse(&["--provision-agent"]),
            Ok(Command::AgentProvision(
                crate::agent::provider::ProviderId::Cursor
            ))
        );
        assert_eq!(
            parse(&["--provision-agent", "codex"]),
            Ok(Command::AgentProvision(
                crate::agent::provider::ProviderId::Codex
            ))
        );
        assert!(parse(&["--provision-agent", "unknown"]).is_err());
    }

    #[test]
    fn rejects_extra_arguments() {
        assert!(parse(&["syntax", "install", "python"]).is_err());
        assert!(parse(&["a", "b"]).is_err());
    }
}
