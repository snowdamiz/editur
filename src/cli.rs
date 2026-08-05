use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Eq, PartialEq)]
pub enum Command {
    Open(Option<PathBuf>),
    Syntax(SyntaxCommand),
    Update,
    #[cfg(windows)]
    FinishUpdate(PathBuf),
    #[cfg(windows)]
    CleanupUpdate(PathBuf),
    Help,
    Version,
}

#[derive(Debug, Eq, PartialEq)]
pub enum SyntaxCommand {
    List,
    Install(OsString),
    Remove(String),
}

pub fn parse_args<I>(args: I) -> Result<Command, String>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = args.into_iter();
    let Some(first) = args.next() else {
        return Ok(Command::Open(None));
    };

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

    if first == "syntax" {
        return parse_syntax(args);
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

fn parse_syntax(mut args: impl Iterator<Item = OsString>) -> Result<Command, String> {
    let action = args
        .next()
        .ok_or_else(|| "missing syntax action; expected list, install, or remove".to_owned())?;
    let action = action
        .to_str()
        .ok_or_else(|| "syntax action must be valid UTF-8".to_owned())?;

    let command = match action {
        "list" => SyntaxCommand::List,
        "install" => SyntaxCommand::Install(
            args.next()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "missing language or package path".to_owned())?,
        ),
        "remove" => {
            let id = args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or_else(|| "missing language ID".to_owned())?;
            if !valid_language_id(&id) {
                return Err(
                    "language ID must contain only lowercase letters, digits, or '-'".into(),
                );
            }
            SyntaxCommand::Remove(id)
        }
        _ => return Err(format!("unknown syntax action `{action}`")),
    };

    if args.next().is_some() {
        Err("too many syntax-command arguments".into())
    } else {
        Ok(Command::Syntax(command))
    }
}

pub(crate) fn valid_language_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
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
    fn parses_syntax_commands_and_rejects_invalid_grammar() {
        assert_eq!(
            parse(&["syntax", "list"]),
            Ok(Command::Syntax(SyntaxCommand::List))
        );
        assert_eq!(
            parse(&["syntax", "install", "python"]),
            Ok(Command::Syntax(SyntaxCommand::Install(OsString::from(
                "python"
            ))))
        );
        assert_eq!(
            parse(&["syntax", "remove", "python"]),
            Ok(Command::Syntax(SyntaxCommand::Remove("python".into())))
        );
        assert!(parse(&["syntax", "install"]).is_err());
        assert!(parse(&["syntax", "remove", "not valid"]).is_err());
        assert!(parse(&["a", "b"]).is_err());
    }
}
