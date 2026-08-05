//! Where Auth2API keeps its settings and how CLI flags override them.
//!
//! Two files live side by side in the config dir: `config.toml` (this module)
//! and `auth.json` (the OAuth credential, see `auth`). Keeping them separate
//! means `logout` can delete the credential without touching the user's
//! server settings.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// The only models the ChatGPT-account backend accepts. Anything else comes
/// back as `400 The '<model>' model is not supported when using Codex with a
/// ChatGPT account`.
///
/// This lives in the config file rather than as a hard-coded constant because
/// OpenAI renames these without warning, and a user hitting a rename should be
/// able to fix it by editing a line instead of waiting for a release.
pub const DEFAULT_MODELS: &[&str] = &["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"];
pub const DEFAULT_MODEL: &str = "gpt-5.6-luna";

fn default_host() -> String {
    "127.0.0.1".to_string()
}
const fn default_port() -> u16 {
    8787
}
fn default_models() -> Vec<String> {
    DEFAULT_MODELS.iter().map(|m| m.to_string()).collect()
}
fn default_model() -> String {
    DEFAULT_MODEL.to_string()
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    /// Bind address. Defaults to loopback on purpose: this server holds a
    /// credential that spends the user's ChatGPT subscription, so it must not
    /// become reachable from the network by accident.
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,

    /// Key clients must send as `Authorization: Bearer <key>`. Empty means no
    /// authentication - only sane while bound to loopback.
    #[serde(default)]
    pub api_key: String,

    /// Optional upstream proxy (http://, https://, socks5://). Empty = direct.
    #[serde(default)]
    pub proxy: String,

    /// Model used when the client asks for one this login cannot serve.
    #[serde(default = "default_model")]
    pub default_model: String,

    /// Models advertised by `/v1/models` and accepted verbatim in requests.
    #[serde(default = "default_models")]
    pub models: Vec<String>,

    /// Client-model -> upstream-model rewrites, applied before the
    /// `models` check. Lets an app hard-coded to `gpt-4o` reach this server
    /// without patching the app.
    #[serde(default)]
    pub model_aliases: std::collections::BTreeMap<String, String>,

    /// Sent as the `instructions` field when the client provides no system
    /// message. The backend behaves noticeably worse with an empty one.
    #[serde(default = "default_instructions")]
    pub default_instructions: String,

    /// USD per 1M tokens, per model, used only to compute the equivalent
    /// list price in usage reports. Empty by default and deliberately so:
    /// a ChatGPT subscription bills nothing per request, so any number here
    /// is a comparison the user chose to make, not a charge this server can
    /// observe. Inventing defaults would produce authoritative-looking
    /// figures that are wrong the moment OpenAI changes a price.
    #[serde(default)]
    pub pricing: crate::stats::PricingTable,
}

fn default_instructions() -> String {
    "You are a helpful assistant.".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            api_key: String::new(),
            proxy: String::new(),
            default_model: default_model(),
            models: default_models(),
            model_aliases: Default::default(),
            default_instructions: default_instructions(),
            pricing: Default::default(),
        }
    }
}

impl Config {
    /// Rewrites a client-requested model into one this login can actually use.
    ///
    /// Falling back instead of erroring is deliberate: the overwhelmingly
    /// common case is a client with `gpt-4o` baked into its defaults, and a
    /// silent 400 from a backend the user never chose to talk to is a much
    /// worse first experience than an answer from the default model.
    pub fn resolve_model(&self, requested: Option<&str>) -> String {
        let requested = requested.map(str::trim).filter(|m| !m.is_empty());
        let Some(requested) = requested else {
            return self.default_model.clone();
        };
        let mapped = self
            .model_aliases
            .get(requested)
            .map(String::as_str)
            .unwrap_or(requested);
        if self.models.iter().any(|m| m == mapped) {
            return mapped.to_string();
        }
        tracing::warn!(
            requested,
            fallback = %self.default_model,
            "model not usable with a ChatGPT-account login, falling back"
        );
        self.default_model.clone()
    }
}

/// Overrides where every Auth2API file lives. Useful for keeping a separate
/// account per project, and what the tests use to stay out of the real one.
pub const HOME_ENV: &str = "AUTH2API_HOME";

pub fn config_dir() -> Result<PathBuf, String> {
    let dir = match std::env::var(HOME_ENV) {
        Ok(path) if !path.trim().is_empty() => PathBuf::from(path),
        _ => directories::ProjectDirs::from("", "", "auth2api")
            .ok_or_else(|| "cannot determine a config directory on this platform".to_string())?
            .config_dir()
            .to_path_buf(),
    };
    fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    Ok(dir)
}

pub fn config_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("config.toml"))
}

pub fn load() -> Result<Config, String> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(Config::default());
    }
    let text = fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    toml::from_str(&text).map_err(|e| format!("cannot parse {}: {e}", path.display()))
}

pub fn save(config: &Config) -> Result<PathBuf, String> {
    let path = config_path()?;
    let text = toml::to_string_pretty(config).map_err(|e| e.to_string())?;
    fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unusable_model_falls_back_instead_of_failing() {
        let config = Config::default();
        assert_eq!(config.resolve_model(Some("gpt-4o")), DEFAULT_MODEL);
        assert_eq!(config.resolve_model(None), DEFAULT_MODEL);
        assert_eq!(config.resolve_model(Some("")), DEFAULT_MODEL);
    }

    #[test]
    fn a_supported_model_is_passed_through() {
        let config = Config::default();
        assert_eq!(config.resolve_model(Some("gpt-5.6-sol")), "gpt-5.6-sol");
    }

    #[test]
    fn aliases_are_applied_before_the_support_check() {
        let mut config = Config::default();
        config
            .model_aliases
            .insert("gpt-4o".into(), "gpt-5.6-terra".into());
        assert_eq!(config.resolve_model(Some("gpt-4o")), "gpt-5.6-terra");
    }

    /// An alias pointing at something the backend still rejects must not
    /// sneak past the check just because it was rewritten.
    #[test]
    fn an_alias_to_an_unusable_model_still_falls_back() {
        let mut config = Config::default();
        config
            .model_aliases
            .insert("fast".into(), "gpt-4o-mini".into());
        assert_eq!(config.resolve_model(Some("fast")), DEFAULT_MODEL);
    }
}
