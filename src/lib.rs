mod auth;
pub mod config;
pub mod sleepme;

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use sleepme::SleepmeClient;

#[derive(Clone)]
pub struct AppState {
    shared_secret: Arc<[u8]>,
    sleepme: SleepmeClient,
    current_setpoint: Arc<RwLock<Option<i16>>>,
}

impl AppState {
    pub fn new(shared_secret: &[u8], sleepme: SleepmeClient) -> Self {
        Self {
            shared_secret: Arc::from(shared_secret),
            sleepme,
            // No confirmed device state until the first successful update.
            current_setpoint: Arc::new(RwLock::new(None)),
        }
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/telemetry", post(telemetry))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::authenticate,
        ))
        .route("/health", get(|| async { StatusCode::OK }))
        .with_state(state)
}

#[derive(Deserialize)]
struct Telemetry {
    heart_rate: u16,
    baseline_hr: Option<u16>,
}

#[derive(Serialize)]
struct TelemetryResponse {
    delta_bpm: i32,
    offset_f: i16,
    target_temperature_f: i16,
    updated: bool,
}

fn target(payload: &Telemetry) -> TelemetryResponse {
    // Signed, widened arithmetic also handles readings below baseline and u16 extremes.
    let delta = i32::from(payload.heart_rate) - i32::from(payload.baseline_hr.unwrap_or(58));
    let offset = if delta >= 15 {
        -5
    } else if delta >= 8 {
        -3
    } else {
        0
    };
    TelemetryResponse {
        delta_bpm: delta,
        offset_f: offset,
        target_temperature_f: 66 + offset,
        updated: false,
    }
}

async fn telemetry(
    State(state): State<AppState>,
    Json(payload): Json<Telemetry>,
) -> Result<Json<TelemetryResponse>, ApiError> {
    let mut result = target(&payload);
    // Serialize the comparison, external write, and cache commit. Releasing this
    // lock before the await would allow duplicate or out-of-order device writes.
    let mut current = state.current_setpoint.write().await;
    if *current != Some(result.target_temperature_f) {
        // A timeout or cancellation could mean Sleepme applied a write we cannot
        // confirm. Invalidate the cache so a later sample reconciles the device.
        *current = None;
        state
            .sleepme
            .set_temperature(result.target_temperature_f)
            .await?;
        *current = Some(result.target_temperature_f);
        result.updated = true;
    }
    Ok(Json(result))
}

#[derive(Debug)]
pub(crate) enum ApiError {
    Unauthorized,
    BadBody,
    BodyTooLarge,
    BodyTimeout,
    Upstream,
    UpstreamTimeout,
    Internal,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "invalid authentication"),
            Self::BadBody => (StatusCode::BAD_REQUEST, "invalid request body"),
            Self::BodyTooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "request body exceeds 4096 bytes",
            ),
            Self::BodyTimeout => (StatusCode::REQUEST_TIMEOUT, "request body timed out"),
            Self::Upstream => (StatusCode::BAD_GATEWAY, "Sleepme update failed"),
            Self::UpstreamTimeout => (StatusCode::GATEWAY_TIMEOUT, "Sleepme update timed out"),
            Self::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "internal server error"),
        };
        #[derive(Serialize)]
        struct ErrorBody {
            error: &'static str,
        }
        (status, Json(ErrorBody { error: message })).into_response()
    }
}

#[cfg(test)]
mod tests;
