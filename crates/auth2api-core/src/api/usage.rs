//! `/v1/usage` - the accounting view. Not part of the OpenAI API; this is
//! Auth2API's own, which is why it takes plain query params.

use super::{check_key, ApiError, AppState};
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct UsageQuery {
    /// How far back to look. Omit for the whole log.
    #[serde(default)]
    pub hours: Option<i64>,
}

pub async fn report(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<UsageQuery>,
) -> Result<Json<crate::stats::Report>, ApiError> {
    check_key(&state.config, &headers)?;
    let records = crate::stats::read_all().map_err(ApiError::upstream)?;
    Ok(Json(crate::stats::report(
        &records,
        &state.config.pricing,
        query.hours,
    )))
}
