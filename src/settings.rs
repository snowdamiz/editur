use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read as _, Write as _},
    path::Path,
};

const MAX_SETTINGS_BYTES: u64 = 64 * 1024;
const MAX_ARGUMENTS: usize = 64;
const MAX_ARGUMENT_BYTES: usize = 4 * 1024;
const MAX_COMMAND_BYTES: usize = 32 * 1024;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    #[serde(skip_serializing_if = "LanguageServerSettings::is_default")]
    pub language_servers: LanguageServerSettings,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct LanguageServerSettings {
    #[serde(skip_serializing_if = "is_true")]
    pub enabled: bool,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub servers: BTreeMap<String, ServerOverride>,
}

impl Default for LanguageServerSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            servers: BTreeMap::new(),
        }
    }
}

impl LanguageServerSettings {
    fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

const fn is_true(value: &bool) -> bool {
    *value
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ServerOverride {
    pub mode: ServerMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ServerMode {
    Auto,
    Custom,
    Off,
}

pub fn load(path: &Path) -> Result<Settings, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Settings::default()),
        Err(error) => return Err(format!("cannot inspect {}: {error}", path.display())),
    };
    if metadata.file_type().is_symlink() {
        return Err(format!("{} is a symbolic link", path.display()));
    }
    if !metadata.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    if metadata.len() > MAX_SETTINGS_BYTES {
        return Err(format!("{} exceeds 64 KiB", path.display()));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    fs::File::open(path)
        .and_then(|file| file.take(MAX_SETTINGS_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if bytes.len() as u64 > MAX_SETTINGS_BYTES {
        return Err(format!("{} exceeds 64 KiB", path.display()));
    }
    let settings = serde_json::from_slice(&bytes)
        .map_err(|error| format!("cannot parse {}: {error}", path.display()))?;
    validate(&settings)?;
    Ok(settings)
}

fn validate(settings: &Settings) -> Result<(), String> {
    for (preset, override_) in &settings.language_servers.servers {
        if override_.args.len() > MAX_ARGUMENTS {
            return Err(format!("{preset} has more than 64 arguments"));
        }
        let command = override_.command.as_deref();
        if override_.mode == ServerMode::Custom
            && command.is_none_or(|command| command.trim().is_empty())
        {
            return Err(format!("{preset} has an empty custom command"));
        }
        if let Some(command) = command
            && override_.mode == ServerMode::Custom
            && !Path::new(command).is_absolute()
            && Path::new(command).components().count() != 1
        {
            return Err(format!(
                "{preset} command must be an absolute path or bare executable name"
            ));
        }
        if command.is_some_and(|command| command.contains('\0'))
            || override_
                .args
                .iter()
                .any(|argument| argument.contains('\0'))
        {
            return Err(format!("{preset} contains a NUL character"));
        }
        if let Some(argument) = override_
            .args
            .iter()
            .find(|argument| argument.len() > MAX_ARGUMENT_BYTES)
        {
            return Err(format!(
                "{preset} argument exceeds 4 KiB ({} bytes)",
                argument.len()
            ));
        }
        let total =
            command.map_or(0, str::len) + override_.args.iter().map(String::len).sum::<usize>();
        if total > MAX_COMMAND_BYTES {
            return Err(format!("{preset} custom command exceeds 32 KiB"));
        }
    }
    Ok(())
}

pub fn save(path: &Path, settings: &Settings) -> Result<(), String> {
    validate(settings)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(format!("{} is a symbolic link", path.display()));
        }
        Ok(metadata) if !metadata.is_file() => {
            return Err(format!("{} is not a regular file", path.display()));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("cannot inspect {}: {error}", path.display())),
    }
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    let mut persisted = settings.clone();
    persisted
        .language_servers
        .servers
        .retain(|_, override_| override_.mode != ServerMode::Auto);
    for override_ in persisted.language_servers.servers.values_mut() {
        if override_.mode == ServerMode::Off {
            override_.command = None;
            override_.args.clear();
        }
    }
    let bytes = serde_json::to_vec_pretty(&persisted)
        .map_err(|error| format!("cannot serialize settings: {error}"))?;
    if bytes.len() as u64 > MAX_SETTINGS_BYTES {
        return Err("serialized settings exceed 64 KiB".into());
    }
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| format!("cannot create settings temporary file: {error}"))?;
    temporary
        .write_all(&bytes)
        .and_then(|()| temporary.flush())
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    temporary
        .persist(path)
        .map_err(|error| format!("cannot replace {}: {}", path.display(), error.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_settings_use_defaults_without_creating_a_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");

        assert_eq!(load(&path).unwrap(), Settings::default());
        assert!(!path.exists());
    }

    #[test]
    fn valid_overrides_round_trip() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let settings = Settings {
            language_servers: LanguageServerSettings {
                enabled: true,
                servers: [(
                    "rust-analyzer".into(),
                    ServerOverride {
                        mode: ServerMode::Custom,
                        command: Some("/opt/tools/rust-analyzer".into()),
                        args: vec!["--log-file".into(), "path with spaces".into()],
                    },
                )]
                .into(),
            },
        };

        save(&path, &settings).unwrap();

        assert_eq!(load(&path).unwrap(), settings);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_settings_are_rejected_without_touching_the_target() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target.json");
        let path = directory.path().join("settings.json");
        fs::write(&target, b"{}").unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();

        assert!(load(&path).unwrap_err().contains("symbolic link"));
        assert_eq!(fs::read(&target).unwrap(), b"{}");
    }

    #[test]
    fn invalid_modes_and_custom_command_bounds_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let invalid = [
            serde_json::json!({"languageServers":{"servers":{"rust-analyzer":{"mode":"wat"}}}}),
            serde_json::json!({"languageServers":{"servers":{"rust-analyzer":{"mode":"custom","command":"  "}}}}),
            serde_json::json!({"languageServers":{"servers":{"rust-analyzer":{"mode":"custom","command":"./rust-analyzer"}}}}),
            serde_json::json!({"languageServers":{"servers":{"rust-analyzer":{"mode":"custom","command":"rust-analyzer\u{0}"}}}}),
            serde_json::json!({"languageServers":{"servers":{"rust-analyzer":{"mode":"custom","command":"rust-analyzer","args":vec!["x"; 65]}}}}),
            serde_json::json!({"languageServers":{"servers":{"rust-analyzer":{"mode":"custom","command":"rust-analyzer","args":["x".repeat(4097)]}}}}),
            serde_json::json!({"languageServers":{"servers":{"rust-analyzer":{"mode":"custom","command":"x".repeat(32 * 1024),"args":["y"]}}}}),
        ];

        for value in invalid {
            fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            assert!(load(&path).is_err(), "accepted {value}");
        }
    }

    #[test]
    fn oversized_and_truncated_settings_are_rejected_without_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let oversized = vec![b' '; MAX_SETTINGS_BYTES as usize + 1];
        fs::write(&path, &oversized).unwrap();
        assert!(load(&path).unwrap_err().contains("64 KiB"));
        assert_eq!(fs::read(&path).unwrap(), oversized);

        let truncated = br#"{"languageServers":{"enabled":false"#.to_vec();
        fs::write(&path, &truncated).unwrap();
        assert!(load(&path).unwrap_err().contains("cannot parse"));
        assert_eq!(fs::read(&path).unwrap(), truncated);
    }

    #[test]
    fn failed_save_leaves_the_existing_file_intact() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        fs::write(&path, b"old settings").unwrap();
        let settings = Settings {
            language_servers: LanguageServerSettings {
                enabled: true,
                servers: [(
                    "rust-analyzer".into(),
                    ServerOverride {
                        mode: ServerMode::Custom,
                        command: Some("x".repeat(32 * 1024 + 1)),
                        args: vec![],
                    },
                )]
                .into(),
            },
        };

        assert!(save(&path, &settings).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"old settings");
    }

    #[test]
    fn serialized_settings_contain_only_deviations_from_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");

        save(&path, &Settings::default()).unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "{}");

        let settings = Settings {
            language_servers: LanguageServerSettings {
                enabled: true,
                servers: [
                    (
                        "rust-analyzer".into(),
                        ServerOverride {
                            mode: ServerMode::Auto,
                            command: Some("ignored".into()),
                            args: vec!["ignored".into()],
                        },
                    ),
                    (
                        "pyright".into(),
                        ServerOverride {
                            mode: ServerMode::Off,
                            command: Some("ignored".into()),
                            args: vec!["ignored".into()],
                        },
                    ),
                ]
                .into(),
            },
        };
        save(&path, &settings).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "{\n  \"languageServers\": {\n    \"servers\": {\n      \"pyright\": {\n        \"mode\": \"off\"\n      }\n    }\n  }\n}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn save_refuses_to_replace_a_symlinked_settings_path() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target.json");
        let path = directory.path().join("settings.json");
        fs::write(&target, b"keep me").unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();

        assert!(save(&path, &Settings::default()).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"keep me");
        assert!(
            fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}
