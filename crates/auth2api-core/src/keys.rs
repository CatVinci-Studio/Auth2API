//! Local API keys - the credentials *clients* present to this server.
//!
//! Not to be confused with `auth`, which is the credential *this server*
//! presents to OpenAI. Several keys can be active at once so that each client
//! (an editor plugin, a script, a phone) gets its own, which is what makes
//! per-key usage meaningful and lets one be revoked without disturbing the
//! rest.
//!
//! Keys live in `keys.json` rather than `config.toml` because the desktop app
//! creates and revokes them at runtime; keeping them out of the hand-edited
//! file means neither writer can clobber the other's changes.

use crate::config::Config;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

pub const PREFIX: &str = "sk-a2a-";
/// Id used for requests authenticated by the single `api_key` in
/// `config.toml`, which predates this file and still works.
pub const LEGACY_ID: &str = "legacy";

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ApiKey {
    pub id: String,
    pub name: String,
    pub secret: String,
    /// Unix seconds.
    pub created_at: i64,
    #[serde(default)]
    pub revoked: bool,
}

impl ApiKey {
    /// What to show in a list. Enough tail to recognise a key you already
    /// have, not enough to use one you don't.
    pub fn masked(&self) -> String {
        let tail: String = self.secret.chars().rev().take(4).collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        format!("{PREFIX}...{tail}")
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct KeyStore {
    #[serde(default)]
    pub keys: Vec<ApiKey>,
}

pub fn keys_path() -> Result<PathBuf, String> {
    Ok(crate::config::config_dir()?.join("keys.json"))
}

pub fn load() -> Result<KeyStore, String> {
    let path = keys_path()?;
    if !path.exists() {
        return Ok(KeyStore::default());
    }
    let text = fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("cannot parse {}: {e}", path.display()))
}

pub fn save(store: &KeyStore) -> Result<(), String> {
    let path = keys_path()?;
    let json = serde_json::to_string_pretty(store).map_err(|e| e.to_string())?;
    fs::write(&path, json).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("cannot chmod {}: {e}", path.display()))?;
    }
    Ok(())
}

fn random_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

/// Mints a new key and persists it. The secret is returned in full exactly
/// once here and stored as-is: this is a local file the user owns, and being
/// able to re-read a key you already generated is worth more than the little
/// a hash would buy against an attacker who can already read `keys.json`.
pub fn create(name: &str) -> Result<ApiKey, String> {
    let mut store = load()?;
    let name = match name.trim() {
        "" => format!("key {}", store.keys.len() + 1),
        name => name.to_string(),
    };
    let key = ApiKey {
        id: format!("k_{}", random_token(6)),
        name,
        secret: format!("{PREFIX}{}", random_token(24)),
        created_at: chrono::Local::now().timestamp(),
        revoked: false,
    };
    store.keys.push(key.clone());
    save(&store)?;
    Ok(key)
}

/// Marks a key unusable while leaving it in the file, so its past usage rows
/// still resolve to a name in reports.
pub fn revoke(id: &str) -> Result<ApiKey, String> {
    let mut store = load()?;
    let key = store
        .keys
        .iter_mut()
        .find(|k| k.id == id)
        .ok_or_else(|| format!("no key with id {id}"))?;
    key.revoked = true;
    let key = key.clone();
    save(&store)?;
    Ok(key)
}

/// Removes a key outright. Its usage rows survive but will render as an
/// unknown id, which is why `revoke` is the better default.
pub fn delete(id: &str) -> Result<(), String> {
    let mut store = load()?;
    let before = store.keys.len();
    store.keys.retain(|k| k.id != id);
    if store.keys.len() == before {
        return Err(format!("no key with id {id}"));
    }
    save(&store)
}

pub fn rename(id: &str, name: &str) -> Result<ApiKey, String> {
    let mut store = load()?;
    let key = store
        .keys
        .iter_mut()
        .find(|k| k.id == id)
        .ok_or_else(|| format!("no key with id {id}"))?;
    key.name = name.to_string();
    let key = key.clone();
    save(&store)?;
    Ok(key)
}

pub fn has_any_active(config: &Config) -> Result<bool, String> {
    Ok(!config.api_key.is_empty() || load()?.keys.iter().any(|k| !k.revoked))
}

/// Compares two secrets without leaking where they first differ through
/// timing. Length is part of the comparison, so a prefix of a real key does
/// not pass.
fn secret_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Who a request is, once authenticated. `None` for the id means the server
/// has no keys configured at all and is accepting anonymous local traffic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller {
    pub key_id: Option<String>,
    pub key_name: Option<String>,
}

impl Caller {
    pub fn anonymous() -> Self {
        Self {
            key_id: None,
            key_name: None,
        }
    }
}

/// Whether this server demands a key at all.
///
/// Deliberately counts revoked keys: revoking your last one must not reopen
/// the server to anonymous callers. Once keys exist, the door stays shut
/// until they are deleted outright, which is an explicit act.
fn requires_auth(config: &Config, store: &KeyStore) -> bool {
    !config.api_key.is_empty() || !store.keys.is_empty()
}

/// Resolves a presented secret against the legacy config key and the key
/// store. Returns `Ok(anonymous)` when no keys exist anywhere, which is the
/// zero-configuration loopback case.
pub fn authenticate(config: &Config, presented: &str) -> Result<Caller, String> {
    let store = load()?;
    if !requires_auth(config, &store) {
        return Ok(Caller::anonymous());
    }

    if !config.api_key.is_empty() && secret_eq(presented, &config.api_key) {
        return Ok(Caller {
            key_id: Some(LEGACY_ID.to_string()),
            key_name: Some("config.toml api_key".to_string()),
        });
    }

    // Every active key is checked even after a match, so the time taken does
    // not reveal the matched key's position in the file.
    let mut found: Option<&ApiKey> = None;
    for key in store.keys.iter().filter(|k| !k.revoked) {
        if secret_eq(presented, &key.secret) {
            found = Some(key);
        }
    }

    match found {
        Some(key) => Ok(Caller {
            key_id: Some(key.id.clone()),
            key_name: Some(key.name.clone()),
        }),
        None => Err("Incorrect API key provided.".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(id: &str, secret: &str, revoked: bool) -> ApiKey {
        ApiKey {
            id: id.into(),
            name: format!("name of {id}"),
            secret: secret.into(),
            created_at: 0,
            revoked,
        }
    }

    /// The store is behind a file, so authentication is exercised through
    /// this pure helper that takes the store directly.
    fn authenticate_with(config: &Config, store: &KeyStore, presented: &str) -> Result<Caller, String> {
        if !requires_auth(config, store) {
            return Ok(Caller::anonymous());
        }
        if !config.api_key.is_empty() && secret_eq(presented, &config.api_key) {
            return Ok(Caller {
                key_id: Some(LEGACY_ID.into()),
                key_name: Some("config.toml api_key".into()),
            });
        }
        let mut found = None;
        for k in store.keys.iter().filter(|k| !k.revoked) {
            if secret_eq(presented, &k.secret) {
                found = Some(k);
            }
        }
        found
            .map(|k| Caller {
                key_id: Some(k.id.clone()),
                key_name: Some(k.name.clone()),
            })
            .ok_or_else(|| "Incorrect API key provided.".to_string())
    }

    #[test]
    fn with_no_keys_anywhere_requests_are_anonymous_rather_than_rejected() {
        let caller = authenticate_with(&Config::default(), &KeyStore::default(), "").unwrap();
        assert_eq!(caller, Caller::anonymous());
    }

    #[test]
    fn a_matching_key_identifies_the_caller() {
        let store = KeyStore {
            keys: vec![key("k_1", "sk-a2a-one", false), key("k_2", "sk-a2a-two", false)],
        };
        let caller = authenticate_with(&Config::default(), &store, "sk-a2a-two").unwrap();
        assert_eq!(caller.key_id.as_deref(), Some("k_2"));
    }

    /// The whole point of revoking rather than deleting: it stops working
    /// immediately but stays resolvable in old reports.
    #[test]
    fn a_revoked_key_stops_authenticating() {
        let store = KeyStore {
            keys: vec![key("k_1", "sk-a2a-one", true)],
        };
        assert!(authenticate_with(&Config::default(), &store, "sk-a2a-one").is_err());
    }

    /// Revoking the last key must not hand the server back to anonymous
    /// callers - the safe direction for that mistake is locked out, not open.
    #[test]
    fn revoking_the_last_key_locks_the_door_rather_than_opening_it() {
        let store = KeyStore {
            keys: vec![key("k_1", "sk-a2a-one", true)],
        };
        assert!(authenticate_with(&Config::default(), &store, "").is_err());
    }

    /// Deleting every key, by contrast, is an explicit reset back to the
    /// zero-configuration state.
    #[test]
    fn deleting_every_key_returns_to_anonymous() {
        let caller = authenticate_with(&Config::default(), &KeyStore::default(), "").unwrap();
        assert_eq!(caller, Caller::anonymous());
    }

    /// Once any key exists, the server must stop accepting anonymous
    /// requests - otherwise creating a key would quietly leave the door open.
    #[test]
    fn creating_a_key_closes_the_anonymous_door() {
        let store = KeyStore {
            keys: vec![key("k_1", "sk-a2a-one", false)],
        };
        assert!(authenticate_with(&Config::default(), &store, "").is_err());
    }

    #[test]
    fn the_legacy_config_key_still_works() {
        let config = Config {
            api_key: "sk-old".into(),
            ..Default::default()
        };
        let caller = authenticate_with(&config, &KeyStore::default(), "sk-old").unwrap();
        assert_eq!(caller.key_id.as_deref(), Some(LEGACY_ID));
    }

    #[test]
    fn a_prefix_of_a_real_key_is_rejected() {
        let store = KeyStore {
            keys: vec![key("k_1", "sk-a2a-longsecret", false)],
        };
        assert!(authenticate_with(&Config::default(), &store, "sk-a2a-long").is_err());
    }

    #[test]
    fn masking_reveals_only_the_tail() {
        let key = key("k_1", "sk-a2a-abcdefgh", false);
        assert_eq!(key.masked(), "sk-a2a-...efgh");
    }
}
