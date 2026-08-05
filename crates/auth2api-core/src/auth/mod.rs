//! "Sign in with ChatGPT" - the PKCE OAuth flow, the credential file, and
//! keeping the access token fresh.
//!
//! This reuses the OpenAI Codex CLI's own public OAuth client id rather than
//! a standard API key, which is what makes the resulting token spend the
//! user's ChatGPT subscription instead of an API-key balance. The token it
//! yields is only accepted by the ChatGPT backend (see `crate::upstream`),
//! not by the public api.openai.com endpoints.

pub mod callback;
pub mod pkce;

use pkce::{decode_base64url, generate_pkce, generate_state, now_ms};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const CALLBACK_PORT: u16 = 1455;
const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const SCOPE: &str = "openid profile email offline_access";
const JWT_CLAIM_PATH: &str = "https://api.openai.com/auth";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Identifies this client to OpenAI in the `originator` param/header. Any
/// short slug works; this mirrors what the Codex CLI sends, just under a
/// different name.
pub const ORIGINATOR: &str = "auth2api";
pub const APP_NAME: &str = "Auth2API";

/// Refresh this long before the token actually expires, so a request that
/// starts just under the wire doesn't finish with a dead token.
const REFRESH_MARGIN_MS: i64 = 120_000;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Credential {
    pub access: String,
    pub refresh: String,
    /// ms since epoch
    pub expires: i64,
    pub account_id: String,
    /// Best-effort, for display only - not every token carries it.
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub plan: Option<String>,
}

pub fn auth_path() -> Result<PathBuf, String> {
    Ok(crate::config::config_dir()?.join("auth.json"))
}

pub fn save(cred: &Credential) -> Result<(), String> {
    let path = auth_path()?;
    let json = serde_json::to_string_pretty(cred).map_err(|e| e.to_string())?;
    fs::write(&path, json).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    // The file holds a live refresh token for the user's ChatGPT account.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("cannot chmod {}: {e}", path.display()))?;
    }
    Ok(())
}

pub fn load() -> Result<Option<Credential>, String> {
    let path = auth_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| format!("cannot parse {}: {e} (delete it and log in again)", path.display()))
}

pub fn logout() -> Result<bool, String> {
    let path = auth_path()?;
    if !path.exists() {
        return Ok(false);
    }
    fs::remove_file(&path).map_err(|e| format!("cannot remove {}: {e}", path.display()))?;
    Ok(true)
}

fn build_authorize_request() -> Result<(String, String, String), String> {
    let (verifier, challenge) = generate_pkce();
    let state = generate_state();

    let mut authorize_url = url::Url::parse(AUTHORIZE_URL).map_err(|e| e.to_string())?;
    authorize_url
        .query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("scope", SCOPE)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &state)
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codex_cli_simplified_flow", "true")
        .append_pair("originator", ORIGINATOR);

    Ok((authorize_url.to_string(), verifier, state))
}

/// Pulls the ChatGPT account id (and, when present, email/plan) out of the
/// access token's JWT payload. The account id is mandatory: every request to
/// the ChatGPT backend must carry it as a header, so a token we cannot read
/// it from is useless and better rejected here than at request time.
fn claims(access_token: &str) -> Option<serde_json::Value> {
    let payload_b64 = access_token.split('.').nth(1)?;
    serde_json::from_slice(&decode_base64url(payload_b64)?).ok()
}

fn credential_from_token(token: TokenResponse) -> Result<Credential, String> {
    let claims = claims(&token.access_token);
    let auth = claims.as_ref().and_then(|c| c.get(JWT_CLAIM_PATH));

    let account_id = auth
        .and_then(|a| a.get("chatgpt_account_id"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            "the token carries no chatgpt_account_id - this login has no ChatGPT account \
             attached to it"
                .to_string()
        })?
        .to_string();

    Ok(Credential {
        access: token.access_token,
        refresh: token.refresh_token,
        expires: now_ms() + token.expires_in * 1000,
        account_id,
        email: claims
            .as_ref()
            .and_then(|c| c.get("email"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        plan: auth
            .and_then(|a| a.get("chatgpt_plan_type"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
    })
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
}

/// Runs the full browser login and persists the credential.
pub async fn login() -> Result<Credential, String> {
    let (authorize_url, verifier, expected_state) = build_authorize_request()?;
    let callback =
        callback::run_login_flow(&authorize_url, CALLBACK_PORT, APP_NAME, LOGIN_TIMEOUT).await?;

    // The state check is what stops a third party from feeding us an
    // authorization code of their choosing via a crafted link to our
    // loopback callback.
    if callback.state != expected_state {
        return Err("OAuth state mismatch - discarding this callback".to_string());
    }

    let cred = exchange_code(&callback.code, &verifier).await?;
    save(&cred)?;
    Ok(cred)
}

async fn exchange_code(code: &str, verifier: &str) -> Result<Credential, String> {
    let res = crate::upstream::http::client()
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT_ID),
            ("code", code),
            ("code_verifier", verifier),
            ("redirect_uri", REDIRECT_URI),
        ])
        .send()
        .await
        .map_err(|e| format!("token exchange request failed: {e}"))?;

    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(format!("token exchange failed ({status}): {body}"));
    }
    credential_from_token(res.json().await.map_err(|e| e.to_string())?)
}

pub async fn refresh(refresh_token: &str) -> Result<Credential, String> {
    let res = crate::upstream::http::client()
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", CLIENT_ID),
        ])
        .send()
        .await
        .map_err(|e| format!("token refresh request failed: {e}"))?;

    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(format!(
            "token refresh failed ({status}): {body}\nRun `auth2api login` to sign in again."
        ));
    }
    credential_from_token(res.json().await.map_err(|e| e.to_string())?)
}

/// Serializes token refreshes. Without it, N concurrent requests arriving on
/// an expired token would each fire their own refresh; OpenAI rotates the
/// refresh token on use, so all but one of those would be spending an
/// already-consumed token and the credential file would end up holding
/// whichever raced last.
static REFRESH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Returns a credential guaranteed valid for the next couple of minutes,
/// refreshing and re-persisting it first if needed.
pub async fn valid_credential() -> Result<Credential, String> {
    let cred = load()?.ok_or_else(|| {
        "not signed in - run `auth2api login` first".to_string()
    })?;
    if cred.expires - now_ms() > REFRESH_MARGIN_MS {
        return Ok(cred);
    }

    let _guard = REFRESH_LOCK.lock().await;
    // Another task may have refreshed while we waited for the lock.
    if let Some(fresh) = load()? {
        if fresh.expires - now_ms() > REFRESH_MARGIN_MS {
            return Ok(fresh);
        }
    }

    tracing::info!("access token expired, refreshing");
    let refreshed = refresh(&cred.refresh).await?;
    save(&refreshed)?;
    Ok(refreshed)
}
