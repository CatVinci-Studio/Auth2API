//! Auth2API core: the ChatGPT-account login, the OpenAI-shaped HTTP surface
//! in front of it, and the accounting underneath.
//!
//! Everything user-facing is built on this - the `auth2api` CLI and the
//! desktop app are both thin shells that call into here, so the two can never
//! drift apart on what a login is or how a request is counted.

pub mod api;
pub mod auth;
pub mod config;
pub mod keys;
pub mod stats;
pub mod translate;
pub mod upstream;

pub use config::Config;

use std::sync::Arc;

/// A bound, running server plus the handle to stop it.
pub struct Server {
    pub addr: std::net::SocketAddr,
    shutdown: tokio::sync::oneshot::Sender<()>,
    join: tokio::task::JoinHandle<()>,
}

impl Server {
    pub async fn stop(self) {
        let _ = self.shutdown.send(());
        let _ = self.join.await;
    }
}

/// Refuses to expose the subscription to the network without a key.
///
/// This process holds a credential that spends the user's ChatGPT plan.
/// Reachable off-loopback with no key, it is an open relay for that plan, so
/// this is an error rather than a warning.
pub fn check_bind_safety(config: &Config) -> Result<(), String> {
    let loopback = matches!(config.host.as_str(), "127.0.0.1" | "::1" | "localhost");
    if loopback || keys::has_any_active(config)? {
        return Ok(());
    }
    Err(format!(
        "refusing to bind {} with no API key configured.\n\
         This server spends your ChatGPT subscription; off-loopback and unauthenticated, \
         anyone who can reach the port can spend it too.\n\
         Create a key first (`auth2api keys new`), or bind 127.0.0.1.",
        config.host
    ))
}

/// Binds and starts the API server. Returns once it is listening, so a caller
/// can report the real address (useful when the configured port was 0).
pub async fn serve(config: Config) -> Result<Server, String> {
    upstream::http::set_proxy(Some(config.proxy.clone()))?;
    check_bind_safety(&config)?;

    let addr = format!("{}:{}", config.host, config.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("cannot bind {addr}: {e}"))?;
    let addr = listener
        .local_addr()
        .map_err(|e| format!("cannot read the bound address: {e}"))?;

    let (shutdown, shutdown_rx) = tokio::sync::oneshot::channel();
    let app = api::router(Arc::new(config));
    let join = tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await;
    });

    Ok(Server {
        addr,
        shutdown,
        join,
    })
}
