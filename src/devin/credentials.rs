use std::fmt;

use serde::{Deserialize, Serialize};

const SERVICE: &str = "io.editur.Editur";
const ACCOUNT: &str = "devin-mcp";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CredentialSource {
    Environment,
    Keyring,
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
        let entry = entry()?;
        let record = match entry.get_password() {
            Ok(record) => record,
            Err(keyring::v1::Error::NoEntry) => return Ok(None),
            Err(_) => return Err("cannot read Devin credentials from the operating system".into()),
        };
        let record: StoredCredentials = serde_json::from_str(&record)
            .map_err(|_| "stored Devin credentials are invalid".to_owned())?;
        Self::new(record.api_key, record.org_id, CredentialSource::Keyring).map(Some)
    }

    pub(crate) fn save(&self) -> Result<(), String> {
        let record = serde_json::to_string(&StoredCredentials {
            api_key: self.api_key.clone(),
            org_id: self.org_id.clone(),
        })
        .map_err(|_| "cannot encode Devin credentials".to_owned())?;
        entry()?
            .set_password(&record)
            .map_err(|_| "cannot save Devin credentials in the operating system".to_owned())?;
        Ok(())
    }

    pub(crate) fn delete() -> Result<(), String> {
        match entry()?.delete_credential() {
            Ok(()) | Err(keyring::v1::Error::NoEntry) => Ok(()),
            Err(_) => Err("cannot remove Devin credentials from the operating system".into()),
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

fn entry() -> Result<keyring::v1::Entry, String> {
    keyring::v1::Entry::new(SERVICE, ACCOUNT)
        .map_err(|_| "the operating-system credential store is unavailable".to_owned())
}

#[cfg(test)]
mod tests {
    #[test]
    fn credentials_never_expose_the_api_key_through_debug_or_errors() {
        let token = "cog_a-secret-that-must-not-leak";
        let credential = super::Credentials::new(
            token.into(),
            Some("org-1".into()),
            super::CredentialSource::Keyring,
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
                super::CredentialSource::Keyring,
            )
            .is_err()
        );
        assert!(
            super::Credentials::new(
                "cog_valid".into(),
                Some("org\nheader".into()),
                super::CredentialSource::Keyring,
            )
            .is_err()
        );
    }
}
