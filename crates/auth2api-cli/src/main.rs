//! The headless Auth2API shell: log in, run the server, manage keys, read the
//! numbers. Everything here is presentation - the behaviour lives in
//! `auth2api-core`, which the desktop app drives through the same functions.

mod render;

use auth2api_core::{auth, config, keys, stats, Config};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "auth2api",
    version,
    about = "Sign in with a ChatGPT account and serve it as a local OpenAI-compatible API"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Sign in with ChatGPT in your browser.
    Login,
    /// Forget the stored ChatGPT credential.
    Logout,
    /// Show who is signed in and where the files live.
    Status,
    /// Run the local API server (the default when no command is given).
    Serve(ServeArgs),
    /// Create and manage the API keys clients use to reach this server.
    Keys {
        #[command(subcommand)]
        action: Option<KeyAction>,
    },
    /// Show token usage, equivalent cost, and when the server gets used.
    Stats(StatsArgs),
    /// Print or create the config file.
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },
}

#[derive(Subcommand)]
enum KeyAction {
    /// List every key, with how much each one has used.
    List,
    /// Mint a new key and print it.
    New {
        /// A label to recognise it by later, e.g. "zed" or "phone".
        name: Option<String>,
    },
    /// Print a key's full secret again.
    Show { id: String },
    /// Give a key a different name.
    Rename { id: String, name: String },
    /// Stop a key working, keeping its usage history readable.
    Revoke { id: String },
    /// Remove a key entirely; its past usage loses its name.
    Delete { id: String },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print the config file path.
    Path,
    /// Write a config file with the current effective settings.
    Init,
    /// Print the effective settings.
    Show,
}

#[derive(clap::Args, Default)]
struct ServeArgs {
    /// Address to bind. Anything other than loopback also requires an API key.
    #[arg(long)]
    host: Option<String>,
    #[arg(long)]
    port: Option<u16>,
    /// Upstream proxy URL (http://, https://, socks5://).
    #[arg(long)]
    proxy: Option<String>,
    /// Model used when a client requests one this login cannot serve.
    #[arg(long)]
    model: Option<String>,
}

#[derive(clap::Args)]
struct StatsArgs {
    /// Only count the last N hours. Omit for everything on record.
    #[arg(long)]
    hours: Option<i64>,
    /// Print the raw report as JSON.
    #[arg(long)]
    json: bool,
    /// Delete the usage log.
    #[arg(long)]
    reset: bool,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "auth2api_core=info,tower_http=warn".into()),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();
    let result = match cli.command {
        Some(Command::Login) => cmd_login().await,
        Some(Command::Logout) => cmd_logout(),
        Some(Command::Status) => cmd_status(),
        Some(Command::Keys { action }) => cmd_keys(action),
        Some(Command::Stats(args)) => cmd_stats(args),
        Some(Command::Config { action }) => cmd_config(action),
        Some(Command::Serve(args)) => cmd_serve(args).await,
        None => cmd_serve(ServeArgs::default()).await,
    };

    if let Err(message) = result {
        eprintln!("\nerror: {message}");
        std::process::exit(1);
    }
}

fn apply_overrides(mut config: Config, args: &ServeArgs) -> Config {
    if let Some(host) = &args.host {
        config.host = host.clone();
    }
    if let Some(port) = args.port {
        config.port = port;
    }
    if let Some(proxy) = &args.proxy {
        config.proxy = proxy.clone();
    }
    if let Some(model) = &args.model {
        config.default_model = model.clone();
    }
    config
}

async fn cmd_login() -> Result<(), String> {
    let cred = auth::login().await?;
    println!("\nSigned in.");
    if let Some(email) = &cred.email {
        println!("  account : {email}");
    }
    println!("  id      : {}", cred.account_id);
    if let Some(plan) = &cred.plan {
        println!("  plan    : {plan}");
    }
    println!("  stored  : {}", auth::auth_path()?.display());
    println!("\nRun `auth2api serve` to start the local API.");
    Ok(())
}

fn cmd_logout() -> Result<(), String> {
    if auth::logout()? {
        println!("Signed out - the stored credential was deleted.");
    } else {
        println!("Not signed in; nothing to do.");
    }
    Ok(())
}

fn cmd_status() -> Result<(), String> {
    let config = config::load()?;
    match auth::load()? {
        Some(cred) => {
            let remaining = (cred.expires - auth::pkce::now_ms()) / 60_000;
            println!("Signed in.");
            if let Some(email) = &cred.email {
                println!("  account       : {email}");
            }
            println!("  id            : {}", cred.account_id);
            if let Some(plan) = &cred.plan {
                println!("  plan          : {plan}");
            }
            // A negative number here is normal, not a problem: the token is
            // refreshed on demand at request time.
            println!("  access token  : {remaining} min until refresh");
        }
        None => println!("Not signed in. Run `auth2api login`."),
    }

    let active = keys::load()?.keys.iter().filter(|k| !k.revoked).count();
    println!("  config        : {}", config::config_path()?.display());
    println!("  keys          : {}", keys::keys_path()?.display());
    println!("  usage log     : {}", stats::log_path()?.display());
    println!("  would bind    : http://{}:{}", config.host, config.port);
    println!(
        "  api keys      : {}",
        match (active, config.api_key.is_empty()) {
            (0, true) => "none - the server accepts any local caller".to_string(),
            (0, false) => "1 (the legacy key in config.toml)".to_string(),
            (n, true) => format!("{n} active"),
            (n, false) => format!("{n} active, plus the legacy key in config.toml"),
        }
    );
    Ok(())
}

fn cmd_keys(action: Option<KeyAction>) -> Result<(), String> {
    match action.unwrap_or(KeyAction::List) {
        KeyAction::List => {
            let store = keys::load()?;
            if store.keys.is_empty() {
                println!("No API keys yet. Create one with `auth2api keys new <name>`.");
                println!(
                    "\nUntil then the server accepts any caller that can reach it, which is \
                     only safe on 127.0.0.1."
                );
                return Ok(());
            }
            let report = stats::report(&stats::read_all()?, &config::load()?.pricing, None);
            render::key_table(&store.keys, &report);
        }
        KeyAction::New { name } => {
            let had_none = !keys::has_any_active(&config::load()?)?;
            let key = keys::create(name.as_deref().unwrap_or(""))?;
            println!("Created key \"{}\" ({})\n", key.name, key.id);
            println!("  {}\n", key.secret);
            println!(
                "Stored in {} - print it again with `auth2api keys show {}`.",
                keys::keys_path()?.display(),
                key.id
            );
            if had_none {
                // Going from zero keys to one flips the server from open to
                // authenticated, which will break any client already pointed
                // at it. Better said now than discovered as a 401.
                println!(
                    "\nNote: this was your first key, so the server now requires one. \
                     Any client already pointed at it needs this key added."
                );
            }
        }
        KeyAction::Show { id } => {
            let store = keys::load()?;
            let key = store
                .keys
                .iter()
                .find(|k| k.id == id)
                .ok_or_else(|| format!("no key with id {id}"))?;
            println!("{}", key.secret);
        }
        KeyAction::Rename { id, name } => {
            let key = keys::rename(&id, &name)?;
            println!("Renamed {} to \"{}\".", key.id, key.name);
        }
        KeyAction::Revoke { id } => {
            let key = keys::revoke(&id)?;
            println!(
                "Revoked \"{}\" ({}). It stops working immediately; its usage history stays.",
                key.name, key.id
            );
        }
        KeyAction::Delete { id } => {
            keys::delete(&id)?;
            println!("Deleted {id}. Its past usage now shows as an unknown key.");
        }
    }
    Ok(())
}

fn cmd_config(action: Option<ConfigAction>) -> Result<(), String> {
    match action.unwrap_or(ConfigAction::Show) {
        ConfigAction::Path => println!("{}", config::config_path()?.display()),
        ConfigAction::Show => println!(
            "{}",
            toml::to_string_pretty(&config::load()?).map_err(|e| e.to_string())?
        ),
        ConfigAction::Init => {
            let path = config::config_path()?;
            if path.exists() {
                return Err(format!(
                    "{} already exists - edit it directly, or delete it first",
                    path.display()
                ));
            }
            let path = config::save(&config::load()?)?;
            println!("Wrote {}", path.display());
            println!(
                "\nTo get an equivalent-cost column in `auth2api stats`, add the list prices \
                 you want to compare against (USD per 1M tokens):\n\n\
                 \x20 [pricing.\"{}\"]\n\
                 \x20 input = 0.0\n\
                 \x20 cached_input = 0.0\n\
                 \x20 output = 0.0\n\n\
                 A ChatGPT subscription bills a flat monthly fee, so these only ever produce \
                 a comparison - not money this server can observe being spent.",
                config::DEFAULT_MODEL
            );
        }
    }
    Ok(())
}

async fn cmd_serve(args: ServeArgs) -> Result<(), String> {
    let config = apply_overrides(config::load()?, &args);

    if auth::load()?.is_none() {
        eprintln!("warning: not signed in - requests will fail until you run `auth2api login`.\n");
    }
    let open_to_all = !keys::has_any_active(&config)?;
    let models = config.models.join(", ");

    let server = auth2api_core::serve(config).await?;
    println!("Auth2API listening on http://{}", server.addr);
    println!("  base URL   : http://{}/v1", server.addr);
    println!(
        "  auth       : {}",
        if open_to_all {
            "none required (create one with `auth2api keys new`)"
        } else {
            "API key required"
        }
    );
    println!("  models     : {models}");
    println!("  endpoints  : /v1/chat/completions  /v1/responses  /v1/models  /v1/usage\n");

    let _ = tokio::signal::ctrl_c().await;
    println!("\nShutting down.");
    server.stop().await;
    Ok(())
}

fn cmd_stats(args: StatsArgs) -> Result<(), String> {
    if args.reset {
        if stats::reset()? {
            println!("Usage log deleted.");
        } else {
            println!("No usage log to delete.");
        }
        return Ok(());
    }

    let config = config::load()?;
    let report = stats::report(&stats::read_all()?, &config.pricing, args.hours);

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
        );
        return Ok(());
    }

    if report.totals.requests == 0 {
        println!("No requests recorded yet ({}).", stats::log_path()?.display());
        return Ok(());
    }
    render::report(&report, args.hours);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_flags_win_over_the_config_file() {
        let config = Config {
            port: 1111,
            default_model: "from-file".into(),
            ..Default::default()
        };
        let merged = apply_overrides(
            config,
            &ServeArgs {
                port: Some(2222),
                ..Default::default()
            },
        );
        assert_eq!(merged.port, 2222);
        // Untouched flags must not clobber the file's values.
        assert_eq!(merged.default_model, "from-file");
    }
}
