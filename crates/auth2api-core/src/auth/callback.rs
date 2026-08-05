//! The localhost half of the browser login: a single-shot HTTP server that
//! exists only long enough to catch OpenAI's redirect.
//!
//! The redirect URI is registered against the upstream OAuth client as
//! `http://localhost:1455/auth/callback`, so the port is not ours to choose -
//! if something else already holds it, the login cannot proceed and says so
//! rather than silently listening somewhere the browser will never reach.

use axum::extract::{Query, State};
use axum::response::Html;
use axum::routing::get;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;

pub struct CallbackResult {
    pub code: String,
    pub state: String,
}

#[derive(Clone)]
struct CallbackState {
    tx: Arc<Mutex<Option<oneshot::Sender<CallbackResult>>>>,
    app_name: String,
}

async fn handle_callback(
    State(state): State<CallbackState>,
    Query(params): Query<HashMap<String, String>>,
) -> Html<String> {
    if let Some(error) = params.get("error") {
        let description = params
            .get("error_description")
            .cloned()
            .unwrap_or_else(|| error.clone());
        return Html(page(&format!("Sign-in failed: {description}"), false));
    }

    let (Some(code), Some(returned_state)) = (params.get("code"), params.get("state")) else {
        return Html(page("Sign-in failed: the redirect carried no authorization code.", false));
    };

    // `take()` makes this single-shot: a browser that replays the redirect (a
    // refresh, a prefetch) must not panic on a consumed sender.
    if let Some(tx) = state.tx.lock().unwrap().take() {
        let _ = tx.send(CallbackResult {
            code: code.clone(),
            state: returned_state.clone(),
        });
    }

    Html(page(
        &format!("Signed in. You can close this tab and return to {}.", state.app_name),
        true,
    ))
}

fn page(message: &str, ok: bool) -> String {
    let accent = if ok { "#10a37f" } else { "#d93025" };
    format!(
        r#"<!doctype html>
<html><head><meta charset="utf-8"><title>Auth2API</title></head>
<body style="font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;
             display:flex;align-items:center;justify-content:center;height:100vh;margin:0;
             background:#fafafa;color:#111">
  <div style="text-align:center;max-width:32rem;padding:2rem">
    <div style="width:3rem;height:3rem;border-radius:50%;background:{accent};margin:0 auto 1.5rem"></div>
    <p style="font-size:1.05rem;line-height:1.6">{message}</p>
  </div>
</body></html>"#
    )
}

/// Binds the callback port, opens the browser, and waits for the redirect.
///
/// The listener is bound *before* the browser opens so a port conflict fails
/// immediately with a clear message, instead of after the user has already
/// typed their password into a page whose redirect will go nowhere.
pub async fn run_login_flow(
    authorize_url: &str,
    port: u16,
    app_name: &str,
    timeout: Duration,
) -> Result<CallbackResult, String> {
    let (tx, rx) = oneshot::channel();
    let router = axum::Router::new()
        .route("/auth/callback", get(handle_callback))
        .with_state(CallbackState {
            tx: Arc::new(Mutex::new(Some(tx))),
            app_name: app_name.to_string(),
        });

    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await.map_err(|e| {
        format!(
            "cannot listen on {addr}: {e}\n\
             This exact port is baked into the upstream OAuth client's redirect URI, \
             so the login cannot use a different one. Stop whatever is holding it \
             (another Codex-style client is the usual culprit) and retry."
        )
    })?;

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await;
    });

    println!("Opening your browser to sign in with ChatGPT...");
    if open::that(authorize_url).is_err() {
        println!("Could not open a browser automatically. Open this URL manually:\n\n{authorize_url}\n");
    }

    let result = tokio::time::timeout(timeout, rx).await;
    let _ = shutdown_tx.send(());
    let _ = server.await;

    match result {
        Ok(Ok(callback)) => Ok(callback),
        Ok(Err(_)) => Err("the login callback channel closed unexpectedly".to_string()),
        Err(_) => Err(format!(
            "login timed out after {}s with no redirect from the browser",
            timeout.as_secs()
        )),
    }
}
