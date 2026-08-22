use std::{
    collections::HashSet,
    fmt, fs,
    io::{self, Write as _},
    path::Path,
};

use serde::{Deserialize, Serialize};

use super::provider::{AccountKey, ProviderId};

const FILE_NAME: &str = "cursor-cloud-credentials.json";
const VERSION: u32 = 1;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_API_KEY_BYTES: usize = 16 * 1024;
const MAX_KEYS: usize = 128;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    account: AccountKey,
    api_key: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Store {
    version: u32,
    entries: Vec<Entry>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            version: VERSION,
            entries: Vec::new(),
        }
    }
}

impl fmt::Debug for Store {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CursorCloudCredentials")
            .field(
                "accounts",
                &self
                    .entries
                    .iter()
                    .map(|entry| entry.account)
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl Store {
    fn load(directory: &Path) -> Result<Self, String> {
        let path = directory.join(FILE_NAME);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(format!("cannot inspect {}: {error}", path.display())),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(format!(
                "{} is not a regular credential file",
                path.display()
            ));
        }
        if metadata.len() > MAX_FILE_BYTES {
            return Err("stored Cursor Cloud credentials are too large".into());
        }
        let bytes =
            fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let store = serde_json::from_slice::<Self>(&bytes)
            .map_err(|_| "stored Cursor Cloud credentials are invalid".to_owned())?;
        store.validate()?;
        Ok(store)
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != VERSION || self.entries.len() > MAX_KEYS {
            return Err("stored Cursor Cloud credentials are invalid".into());
        }
        let mut accounts = HashSet::new();
        if self.entries.iter().any(|entry| {
            entry.account.provider != ProviderId::Cursor
                || !accounts.insert(entry.account)
                || validate_api_key(&entry.api_key).is_err()
        }) {
            return Err("stored Cursor Cloud credentials are invalid".into());
        }
        Ok(())
    }

    fn save(&self, directory: &Path) -> Result<(), String> {
        self.validate()?;
        fs::create_dir_all(directory)
            .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
        let path = directory.join(FILE_NAME);
        match fs::symlink_metadata(&path) {
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
        let bytes = serde_json::to_vec(self)
            .map_err(|_| "cannot encode Cursor Cloud credentials".to_owned())?;
        let mut temporary = tempfile::NamedTempFile::new_in(directory)
            .map_err(|error| format!("cannot create credential temporary file: {error}"))?;
        temporary
            .write_all(&bytes)
            .and_then(|()| temporary.flush())
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        temporary
            .persist(&path)
            .map_err(|error| format!("cannot replace {}: {}", path.display(), error.error))?;
        Ok(())
    }
}

fn validate_api_key(api_key: &str) -> Result<&str, String> {
    let api_key = api_key.trim();
    if api_key.len() < 8
        || api_key.len() > MAX_API_KEY_BYTES
        || !api_key.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err("Cursor API key is invalid".into());
    }
    Ok(api_key)
}

pub(crate) fn configured_from(directory: &Path) -> Result<HashSet<AccountKey>, String> {
    Ok(Store::load(directory)?
        .entries
        .into_iter()
        .map(|entry| entry.account)
        .collect())
}

pub(crate) fn load_from(directory: &Path, account: AccountKey) -> Result<Option<String>, String> {
    Ok(Store::load(directory)?
        .entries
        .into_iter()
        .find(|entry| entry.account == account)
        .map(|entry| entry.api_key))
}

pub(crate) fn save_to(directory: &Path, account: AccountKey, api_key: &str) -> Result<(), String> {
    if account.provider != ProviderId::Cursor {
        return Err("Cursor Cloud credentials require a Cursor account".into());
    }
    let api_key = validate_api_key(api_key)?.to_owned();
    let mut store = Store::load(directory)?;
    if let Some(entry) = store
        .entries
        .iter_mut()
        .find(|entry| entry.account == account)
    {
        entry.api_key = api_key;
    } else {
        if store.entries.len() == MAX_KEYS {
            return Err(format!("store at most {MAX_KEYS} Cursor API keys"));
        }
        store.entries.push(Entry { account, api_key });
    }
    store.save(directory)
}

pub(crate) fn remove_from(directory: &Path, account: AccountKey) -> Result<(), String> {
    let mut store = Store::load(directory)?;
    store.entries.retain(|entry| entry.account != account);
    store.save(directory)
}

#[cfg(test)]
mod tests {
    use crate::agent::provider::{AccountKey, ProviderId};

    #[test]
    fn cursor_cloud_keys_round_trip_per_account_without_entering_debug_output() {
        let directory = tempfile::tempdir().unwrap();
        let account = AccountKey {
            provider: ProviderId::Cursor,
            account_id: 7,
        };
        let secret = "cursor-secret-for-cloud-progress";

        super::save_to(directory.path(), account, secret).unwrap();
        assert_eq!(
            super::load_from(directory.path(), account)
                .unwrap()
                .as_deref(),
            Some(secret)
        );
        assert_eq!(
            super::configured_from(directory.path()).unwrap(),
            std::collections::HashSet::from([account])
        );
        assert!(!format!("{:?}", super::Store::load(directory.path()).unwrap()).contains(secret));

        super::save_to(directory.path(), account, "replacement-cloud-key").unwrap();
        assert_eq!(
            super::load_from(directory.path(), account)
                .unwrap()
                .as_deref(),
            Some("replacement-cloud-key")
        );

        super::remove_from(directory.path(), account).unwrap();
        assert_eq!(super::load_from(directory.path(), account).unwrap(), None);
    }
}
