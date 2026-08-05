//! The desktop shell.
//!
//! Every command here is a thin wrapper over `auth2api-core` - the same
//! functions the CLI calls - so the app and `auth2api serve` can never
//! disagree about what a login is, which keys are valid, or how usage is
//! counted. The window is a view onto the same files, not a second
//! implementation.

use auth2api_core::{auth, config, keys, stats, Config, Server};
use serde::Serialize;
use std::sync::Mutex;
use tauri::State;

/// The running server, if any. `None` means stopped; the app starts stopped
/// so that launching it never silently opens a port.
#[derive(Default)]
struct ServerState(Mutex<Option<Server>>);

#[derive(Serialize)]
struct Account {
    email: Option<String>,
    account_id: String,
    plan: Option<String>,
}

#[derive(Serialize)]
struct AppStatus {
    signed_in: bool,
    account: Option<Account>,
    running: bool,
    address: Option<String>,
    host: String,
    port: u16,
    models: Vec<String>,
    default_model: String,
    /// True while no key exists at all, i.e. any caller is accepted.
    open_to_anyone: bool,
    /// True when bound to something other than loopback.
    open_host: bool,
    /// Addresses another machine could actually dial, best first. `0.0.0.0`
    /// is what the socket binds but not something anyone can connect to, so
    /// the window needs these to show instead.
    lan_addrs: Vec<LanAddr>,
    config_path: String,
}

fn status_of(server: &Option<Server>) -> Result<AppStatus, String> {
    let config = config::load()?;
    let cred = auth::load()?;
    Ok(AppStatus {
        signed_in: cred.is_some(),
        account: cred.map(|c| Account {
            email: c.email,
            account_id: c.account_id,
            plan: c.plan,
        }),
        running: server.is_some(),
        address: server.as_ref().map(|s| s.addr.to_string()),
        open_to_anyone: !keys::has_any_active(&config)?,
        open_host: !matches!(config.host.as_str(), "127.0.0.1" | "::1" | "localhost"),
        lan_addrs: lan_addrs(),
        host: config.host.clone(),
        port: config.port,
        models: config.models.clone(),
        default_model: config.default_model.clone(),
        config_path: config::config_path()?.display().to_string(),
    })
}

#[derive(Serialize)]
struct LanAddr {
    iface: String,
    ip: String,
}

/// True for interface names that are VPN or virtual tunnels.
///
/// These have to be filtered out rather than ranked down: with a VPN up, the
/// tunnel is usually the route to the internet *and* carries a private-looking
/// address, so both "ask the routing table" and "prefer a private range" pick
/// it - and it is precisely the address no one on the local network can reach.
fn is_tunnel(name: &str) -> bool {
    const TUNNELS: [&str; 7] = ["utun", "tun", "tap", "ppp", "ipsec", "wg", "zt"];
    TUNNELS.iter().any(|prefix| name.starts_with(prefix))
}

/// Addresses this machine can be reached at from the local network, best
/// first. Physical interfaces (`en0`, `eth0`, `wlan0`) are preferred over
/// bridges and virtual adapters, which are commonly up but unrouted.
fn lan_addrs() -> Vec<LanAddr> {
    let Ok(interfaces) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    let mut found: Vec<(u8, LanAddr)> = interfaces
        .into_iter()
        .filter(|i| !i.is_loopback() && !is_tunnel(&i.name))
        .filter_map(|i| match i.addr.ip() {
            // IPv6 link-local needs a scope id to dial and would only confuse
            // someone copying an address into another machine.
            std::net::IpAddr::V4(v4) if !v4.is_link_local() => Some((
                u8::from(!matches!(&i.name[..2], "en" | "et" | "wl")),
                LanAddr {
                    iface: i.name,
                    ip: v4.to_string(),
                },
            )),
            _ => None,
        })
        .collect();
    found.sort_by_key(|(rank, _)| *rank);
    found.into_iter().map(|(_, addr)| addr).collect()
}

#[tauri::command]
fn status(state: State<ServerState>) -> Result<AppStatus, String> {
    status_of(&state.0.lock().unwrap())
}

#[tauri::command]
async fn login() -> Result<(), String> {
    auth::login().await.map(|_| ())
}

#[tauri::command]
fn logout(state: State<ServerState>) -> Result<AppStatus, String> {
    auth::logout()?;
    status_of(&state.0.lock().unwrap())
}

/// Starts the server on the given address, persisting it as the new default
/// so the CLI and the next launch agree with what was just chosen here.
///
/// `open` swaps loopback for 0.0.0.0. Whether that is allowed is not decided
/// here: `serve` refuses to expose the subscription without an API key, and
/// its refusal is what the window shows.
#[tauri::command]
async fn start(state: State<'_, ServerState>, port: u16, open: bool) -> Result<AppStatus, String> {
    if state.0.lock().unwrap().is_some() {
        return Err("the server is already running".to_string());
    }

    let mut config = config::load()?;
    config.port = port;
    config.host = if open { "0.0.0.0" } else { "127.0.0.1" }.to_string();
    config::save(&config)?;

    let server = auth2api_core::serve(config).await?;
    let mut guard = state.0.lock().unwrap();
    *guard = Some(server);
    status_of(&guard)
}

#[tauri::command]
async fn stop(state: State<'_, ServerState>) -> Result<AppStatus, String> {
    // Taken out of the lock before awaiting, because the guard is not Send
    // and shutdown has to await the server task.
    let server = state.0.lock().unwrap().take();
    if let Some(server) = server {
        server.stop().await;
    }
    let guard = state.0.lock().unwrap();
    status_of(&guard)
}

#[tauri::command]
fn usage(hours: Option<i64>) -> Result<stats::Report, String> {
    let config = config::load()?;
    Ok(stats::report(&stats::read_all()?, &config.pricing, hours))
}

#[tauri::command]
fn usage_reset() -> Result<(), String> {
    stats::reset().map(|_| ())
}

#[derive(Serialize)]
struct KeyRow {
    id: String,
    name: String,
    secret: String,
    masked: String,
    created_at: i64,
    revoked: bool,
    requests: u64,
    total_tokens: u64,
    last_used: Option<String>,
}

#[tauri::command]
fn list_keys() -> Result<Vec<KeyRow>, String> {
    let config = config::load()?;
    let report = stats::report(&stats::read_all()?, &config.pricing, None);
    Ok(keys::load()?
        .keys
        .into_iter()
        .map(|key| {
            let usage = report.by_key.iter().find(|b| b.key == key.id);
            KeyRow {
                masked: key.masked(),
                requests: usage.map(|u| u.requests).unwrap_or(0),
                total_tokens: usage.map(|u| u.total_tokens).unwrap_or(0),
                last_used: usage.and_then(|u| u.last_used.clone()),
                id: key.id,
                name: key.name,
                secret: key.secret,
                created_at: key.created_at,
                revoked: key.revoked,
            }
        })
        .collect())
}

#[tauri::command]
fn create_key(name: String) -> Result<String, String> {
    keys::create(&name).map(|key| key.secret)
}

#[tauri::command]
fn revoke_key(id: String) -> Result<(), String> {
    keys::revoke(&id).map(|_| ())
}

#[tauri::command]
fn delete_key(id: String) -> Result<(), String> {
    keys::delete(&id)
}

#[tauri::command]
fn rename_key(id: String, name: String) -> Result<(), String> {
    keys::rename(&id, &name).map(|_| ())
}

/// Saves the settings the window can change.
///
/// Each field is optional and an omitted one is left exactly as written -
/// otherwise saving a port would silently overwrite a proxy or model the
/// window never showed and the user set by hand in `config.toml`.
#[tauri::command]
fn save_settings(
    port: Option<u16>,
    default_model: Option<String>,
    proxy: Option<String>,
) -> Result<Config, String> {
    let mut config = config::load()?;
    if let Some(port) = port {
        config.port = port;
    }
    if let Some(model) = default_model {
        config.default_model = model;
    }
    if let Some(proxy) = proxy {
        config.proxy = proxy;
    }
    config::save(&config)?;
    Ok(config)
}

/// Widens the window when the stats pane opens and narrows it again when it
/// closes, so the collapsed app stays as small as it looks.
///
/// Driven from Rust because a window resize invoked from the frontend would
/// need its own capability grant, and this is the only sizing the app does.
#[tauri::command]
fn set_expanded(window: tauri::Window, expanded: bool) -> Result<(), String> {
    let width = if expanded { EXPANDED_WIDTH } else { COLLAPSED_WIDTH };
    window
        .set_size(tauri::LogicalSize::new(width, WINDOW_HEIGHT))
        .map_err(|e| e.to_string())?;

    // Growing rightwards from near the screen edge would put the pane that
    // just opened off-screen, where it cannot be read or scrolled to. Windows
    // do not reposition themselves on resize, so this nudges it back.
    let (Ok(Some(monitor)), Ok(position)) = (window.current_monitor(), window.outer_position())
    else {
        return Ok(());
    };
    let scale = monitor.scale_factor();
    let screen = monitor.size().to_logical::<f64>(scale);
    let origin = monitor.position().to_logical::<f64>(scale);
    let position = position.to_logical::<f64>(scale);

    let overflow = (position.x + width) - (origin.x + screen.width);
    if overflow > 0.0 {
        let x = (position.x - overflow).max(origin.x);
        window
            .set_position(tauri::LogicalPosition::new(x, position.y))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

const COLLAPSED_WIDTH: f64 = 380.0;
const EXPANDED_WIDTH: f64 = 940.0;
const WINDOW_HEIGHT: f64 = 580.0;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(ServerState::default())
        .invoke_handler(tauri::generate_handler![
            status,
            login,
            logout,
            start,
            stop,
            usage,
            usage_reset,
            list_keys,
            create_key,
            revoke_key,
            delete_key,
            rename_key,
            save_settings,
            set_expanded,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Auth2API window");
}
