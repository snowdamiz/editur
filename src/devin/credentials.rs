use std::{
    fmt, fs,
    io::{self, Write as _},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "devin-credentials.json";
const MAX_STORED_BYTES: u64 = 20 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CredentialSource {
    Environment,
    Stored,
}

#[derive(Clone)]
pub(crate) struct Credentials {
    api_key: String,
    org_id: Option<String>,
    source: CredentialSource,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Credentials")
            .field("api_key", &"[REDACTED]")
            .field("org_id", &self.org_id.as_ref().map(|_| "[SET]"))
            .field("source", &self.source)
            .finish()
    }
}

impl Credentials {
    pub(crate) fn new(
        api_key: String,
        org_id: Option<String>,
        source: CredentialSource,
    ) -> Result<Self, String> {
        let api_key = api_key.trim().to_owned();
        if !api_key.starts_with("cog_")
            || api_key.len() <= 4
            || api_key.len() > 16 * 1024
            || api_key.chars().any(char::is_control)
        {
            return Err("Devin API keys must start with cog_".into());
        }
        let org_id = org_id.and_then(|value| {
            let value = value.trim().to_owned();
            (!value.is_empty()).then_some(value)
        });
        if org_id
            .as_ref()
            .is_some_and(|value| value.len() > 1_024 || value.chars().any(char::is_control))
        {
            return Err("The Devin organization ID is invalid".into());
        }
        Ok(Self {
            api_key,
            org_id,
            source,
        })
    }

    pub(crate) fn load() -> Result<Option<Self>, String> {
        if let Ok(api_key) = std::env::var("DEVIN_API_KEY")
            && !api_key.trim().is_empty()
        {
            return Self::new(
                api_key,
                std::env::var("DEVIN_ORG_ID").ok(),
                CredentialSource::Environment,
            )
            .map(Some);
        }
        Self::load_from(&credential_path()?)
    }

    pub(crate) fn save(&self) -> Result<(), String> {
        self.save_to(&credential_path()?)
    }

    fn load_from(path: &Path) -> Result<Option<Self>, String> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("cannot inspect {}: {error}", path.display())),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(format!(
                "{} is not a regular credential file",
                path.display()
            ));
        }
        if metadata.len() > MAX_STORED_BYTES {
            return Err("stored Devin credentials are too large".into());
        }
        let record = fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let record: StoredCredentials = serde_json::from_str(&record)
            .map_err(|_| "stored Devin credentials are invalid".to_owned())?;
        Self::new(record.api_key, record.org_id, CredentialSource::Stored).map(Some)
    }

    fn save_to(&self, path: &Path) -> Result<(), String> {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(format!(
                    "{} is not a regular credential file",
                    path.display()
                ));
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
        let record = serde_json::to_vec(&StoredCredentials {
            api_key: self.api_key.clone(),
            org_id: self.org_id.clone(),
        })
        .map_err(|_| "cannot encode Devin credentials".to_owned())?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)
            .map_err(|error| format!("cannot create credential temporary file: {error}"))?;
        temporary
            .write_all(&record)
            .and_then(|()| temporary.flush())
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        temporary
            .persist(path)
            .map_err(|error| format!("cannot replace {}: {}", path.display(), error.error))?;
        Ok(())
    }

    pub(crate) fn delete() -> Result<(), String> {
        Self::delete_from(&credential_path()?)
    }

    fn delete_from(path: &Path) -> Result<(), String> {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => Err(
                format!("{} is not a regular credential file", path.display()),
            ),
            Ok(_) => fs::remove_file(path)
                .map_err(|error| format!("cannot remove {}: {error}", path.display())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("cannot inspect {}: {error}", path.display())),
        }
    }

    #[cfg(feature = "network")]
    pub(crate) fn api_key(&self) -> &str {
        &self.api_key
    }

    #[cfg(feature = "network")]
    pub(crate) fn org_id(&self) -> Option<&str> {
        self.org_id.as_deref()
    }

    pub(crate) const fn source(&self) -> CredentialSource {
        self.source
    }

    pub(crate) fn with_org_id(&self, org_id: String) -> Result<Self, String> {
        Self::new(self.api_key.clone(), Some(org_id), self.source)
    }
}

#[derive(Deserialize, Serialize)]
struct StoredCredentials {
    api_key: String,
    org_id: Option<String>,
}

fn credential_path() -> Result<PathBuf, String> {
    crate::data_dir().map(|directory| directory.join(FILE_NAME))
}

#[cfg(test)]
mod tests {
    #[test]
    fn stored_credentials_round_trip_through_one_local_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("devin-credentials.json");
        let credentials = super::Credentials::new(
            "cog_local-test".into(),
            Some("org-1".into()),
            super::CredentialSource::Stored,
        )
        .unwrap();

        credentials.save_to(&path).unwrap();
        let loaded = super::Credentials::load_from(&path).unwrap().unwrap();

        assert_eq!(
            (
                loaded.api_key.as_str(),
                loaded.org_id.as_deref(),
                loaded.source
            ),
            (
                "cog_local-test",
                Some("org-1"),
                super::CredentialSource::Stored
            )
        );
    }

    #[test]
    fn credentials_never_expose_the_api_key_through_debug_or_errors() {
        let token = "cog_a-secret-that-must-not-leak";
        let credential = super::Credentials::new(
            token.into(),
            Some("org-1".into()),
            super::CredentialSource::Stored,
        )
        .unwrap();

        assert!(!format!("{credential:?}").contains(token));
    }

    #[test]
    fn credentials_reject_unbounded_or_header_unsafe_values() {
        assert!(
            super::Credentials::new(
                format!("cog_{}", "x".repeat(20_000)),
                None,
                super::CredentialSource::Stored,
            )
            .is_err()
        );
        assert!(
            super::Credentials::new(
                "cog_valid".into(),
                Some("org\nheader".into()),
                super::CredentialSource::Stored,
            )
            .is_err()
        );
    }
}
