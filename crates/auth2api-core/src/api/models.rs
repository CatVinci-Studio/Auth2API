//! `/v1/models` - the list many clients probe at startup to decide whether an
//! endpoint is usable at all.

use super::{check_key, ApiError, AppState};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde_json::{json, Value};

fn entry(id: &str) -> Value {
    json!({
        "id": id,
        "object": "model",
        // The real creation date is not observable through this backend, and
        // clients only ever sort or display it.
        "created": 0,
        "owned_by": "openai",
    })
}

pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    check_key(&state.config, &headers)?;
    Ok(Json(json!({
        "object": "list",
        "data": state.config.models.iter().map(|m| entry(m)).collect::<Vec<_>>(),
    })))
}

pub async fn retrieve(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(model): Path<String>,
) -> Result<Json<Value>, ApiError> {
    check_key(&state.config, &headers)?;
    if !state.config.models.contains(&model) {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "invalid_request_error",
            format!("The model '{model}' does not exist or is not available to this login."),
        ));
    }
    Ok(Json(entry(&model)))
}
