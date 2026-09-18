use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    body::{Body, to_bytes},
    extract::State,
    http::{HeaderMap, Request, StatusCode, header::AUTHORIZATION},
    routing::patch,
};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use tokio::{sync::Mutex, task::JoinHandle};
use tower::ServiceExt;

use super::*;

const SECRET: &[u8] = b"test-shared-secret";

#[derive(Clone)]
struct MockState {
    requests: Arc<Mutex<Vec<Value>>>,
    statuses: Arc<Mutex<VecDeque<StatusCode>>>,
    delay: Duration,
}

struct Fixture {
    app: Router,
    state: AppState,
    mock: MockState,
    server: JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Fixture {
    async fn new(statuses: &[StatusCode], delay: Duration, timeout: Duration) -> Self {
        let mock = MockState {
            requests: Arc::default(),
            statuses: Arc::new(Mutex::new(statuses.iter().copied().collect())),
            delay,
        };
        let api = Router::new()
            .route("/v1/devices/test-device", patch(mock_update))
            .with_state(mock.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/v1/devices/test-device",
            listener.local_addr().unwrap()
        );
        let server = tokio::spawn(async move { axum::serve(listener, api).await.unwrap() });
        let state = AppState::new(SECRET, SleepmeClient::mock(url.parse().unwrap(), timeout));
        Self {
            app: router(state.clone()),
            state,
            mock,
            server,
        }
    }

    async fn healthy() -> Self {
        Self::new(&[], Duration::ZERO, Duration::from_secs(2)).await
    }

    async fn send(&self, body: &str) -> (StatusCode, Value) {
        let response = self.app.clone().oneshot(signed(body)).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }
}

async fn mock_update(
    State(state): State<MockState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> StatusCode {
    assert_eq!(headers[AUTHORIZATION], "Bearer test-token");
    assert_eq!(headers["content-type"], "application/json");
    state.requests.lock().await.push(body);
    tokio::time::sleep(state.delay).await;
    state
        .statuses
        .lock()
        .await
        .pop_front()
        .unwrap_or(StatusCode::OK)
}

fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn signature(timestamp: &str, body: &str, secret: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).unwrap();
    mac.update(timestamp.as_bytes());
    mac.update(b".");
    mac.update(body.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn signed_at(body: &str, timestamp: &str) -> Request<Body> {
    Request::post("/api/v1/telemetry")
        .header("content-type", "application/json")
        .header("x-timestamp", timestamp)
        .header("x-signature", signature(timestamp, body, SECRET))
        .body(Body::from(body.to_owned()))
        .unwrap()
}

fn signed(body: &str) -> Request<Body> {
    signed_at(body, &timestamp().to_string())
}

#[test]
fn cooling_thresholds_and_signed_extremes() {
    for (hr, baseline, delta, offset, temperature) in [
        (58, None, 0, 0, 66),
        (65, None, 7, 0, 66),
        (66, None, 8, -3, 63),
        (72, None, 14, -3, 63),
        (73, None, 15, -5, 61),
        (100, None, 42, -5, 61),
        (40, None, -18, 0, 66),
        (78, Some(70), 8, -3, 63),
        (0, Some(u16::MAX), -65535, 0, 66),
        (u16::MAX, Some(0), 65535, -5, 61),
    ] {
        let result = target(&Telemetry {
            heart_rate: hr,
            baseline_hr: baseline,
        });
        assert_eq!(
            (
                result.delta_bpm,
                result.offset_f,
                result.target_temperature_f
            ),
            (delta, offset, temperature)
        );
    }
}

#[tokio::test]
async fn health_is_public_and_does_not_call_sleepme() {
    let fixture = Fixture::healthy().await;
    let response = fixture
        .app
        .clone()
        .oneshot(Request::get("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(fixture.mock.requests.lock().await.is_empty());
}

#[tokio::test]
async fn valid_raw_body_is_restored_for_json_and_custom_baseline_is_used() {
    let fixture = Fixture::healthy().await;
    let (status, result) = fixture
        .send("{\n  \"heart_rate\": 80, \"baseline_hr\": 70\n}")
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        result,
        json!({"delta_bpm":10,"offset_f":-3,"target_temperature_f":63,"updated":true})
    );
    assert_eq!(
        *fixture.mock.requests.lock().await,
        vec![json!({"set_temperature_f":63})]
    );
}

#[tokio::test]
async fn first_baseline_is_sent_then_only_changes_are_sent() {
    let fixture = Fixture::healthy().await;
    for (hr, updated) in [
        (58, true),
        (65, false),
        (66, true),
        (72, false),
        (73, true),
        (80, false),
        (58, true),
    ] {
        let (status, result) = fixture.send(&format!("{{\"heart_rate\":{hr}}}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(result["updated"], updated);
    }
    assert_eq!(
        *fixture.mock.requests.lock().await,
        vec![
            json!({"set_temperature_f":66}),
            json!({"set_temperature_f":63}),
            json!({"set_temperature_f":61}),
            json!({"set_temperature_f":66}),
        ]
    );
}

#[tokio::test]
async fn simultaneous_same_targets_make_one_external_call() {
    let fixture = Fixture::new(&[], Duration::from_millis(30), Duration::from_secs(2)).await;
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..24 {
        let app = fixture.app.clone();
        tasks.spawn(async move { app.oneshot(signed(r#"{"heart_rate":80}"#)).await.unwrap() });
    }
    let mut updates = 0;
    while let Some(response) = tasks.join_next().await {
        let response = response.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        let result: Value = serde_json::from_slice(&bytes).unwrap();
        updates += usize::from(result["updated"] == true);
    }
    assert_eq!(updates, 1);
    assert_eq!(fixture.mock.requests.lock().await.len(), 1);
}

#[tokio::test]
async fn failed_update_is_retried() {
    let fixture = Fixture::new(
        &[StatusCode::SERVICE_UNAVAILABLE, StatusCode::OK],
        Duration::ZERO,
        Duration::from_secs(2),
    )
    .await;
    assert_eq!(
        fixture.send(r#"{"heart_rate":80}"#).await.0,
        StatusCode::BAD_GATEWAY
    );
    assert_eq!(*fixture.state.current_setpoint.read().await, None);
    let (status, result) = fixture.send(r#"{"heart_rate":80}"#).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["updated"], true);
    assert_eq!(fixture.mock.requests.lock().await.len(), 2);
}

#[tokio::test]
async fn uncertain_change_invalidates_previous_cached_temperature() {
    let fixture = Fixture::new(
        &[StatusCode::OK, StatusCode::BAD_GATEWAY, StatusCode::OK],
        Duration::ZERO,
        Duration::from_secs(2),
    )
    .await;
    assert_eq!(fixture.send(r#"{"heart_rate":58}"#).await.0, StatusCode::OK);
    assert_eq!(
        fixture.send(r#"{"heart_rate":80}"#).await.0,
        StatusCode::BAD_GATEWAY
    );
    let (_, result) = fixture.send(r#"{"heart_rate":58}"#).await;
    assert_eq!(result["updated"], true);
    assert_eq!(fixture.mock.requests.lock().await.len(), 3);
}

#[tokio::test]
async fn timeout_and_redirect_do_not_commit_setpoint() {
    let fixture = Fixture::new(&[], Duration::from_millis(200), Duration::from_millis(20)).await;
    assert_eq!(
        fixture.send(r#"{"heart_rate":80}"#).await.0,
        StatusCode::GATEWAY_TIMEOUT
    );
    assert_eq!(*fixture.state.current_setpoint.read().await, None);
    let fixture = Fixture::new(
        &[StatusCode::TEMPORARY_REDIRECT],
        Duration::ZERO,
        Duration::from_secs(2),
    )
    .await;
    assert_eq!(
        fixture.send(r#"{"heart_rate":80}"#).await.0,
        StatusCode::BAD_GATEWAY
    );
    assert_eq!(*fixture.state.current_setpoint.read().await, None);
}

#[tokio::test]
async fn authentication_rejects_missing_malformed_and_duplicate_headers() {
    let fixture = Fixture::healthy().await;
    let body = r#"{"heart_rate":80}"#;
    for header in ["x-timestamp", "x-signature"] {
        let mut request = signed(body);
        request.headers_mut().remove(header);
        assert_eq!(
            fixture.app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let mut request = signed(body);
        request
            .headers_mut()
            .append(header, "duplicate".parse().unwrap());
        assert_eq!(
            fixture.app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }
    for value in ["", "abc", "-1", "+123", "18446744073709551616", "123.5"] {
        let request = signed_at(body, value);
        assert_eq!(
            fixture.app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }
    for value in [
        "",
        "zz",
        "1234",
        &"g".repeat(64),
        &"0".repeat(64),
        &"0".repeat(66),
    ] {
        let mut request = signed(body);
        request
            .headers_mut()
            .insert("x-signature", value.parse().unwrap());
        assert_eq!(
            fixture.app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert!(fixture.mock.requests.lock().await.is_empty());
}

#[tokio::test]
async fn stale_future_tampered_and_wrong_secret_requests_are_rejected() {
    let fixture = Fixture::healthy().await;
    let body = r#"{"heart_rate":80}"#;
    for time in [timestamp() - 31, timestamp() + 120, u64::MAX] {
        assert_eq!(
            fixture
                .app
                .clone()
                .oneshot(signed_at(body, &time.to_string()))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    let mut request = signed(body);
    *request.body_mut() = Body::from(r#"{"heart_rate":81}"#);
    assert_eq!(
        fixture.app.clone().oneshot(request).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let time = timestamp().to_string();
    let mut request = signed_at(body, &time);
    request.headers_mut().insert(
        "x-signature",
        signature(&time, body, b"wrong-secret").parse().unwrap(),
    );
    assert_eq!(
        fixture.app.clone().oneshot(request).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert!(fixture.mock.requests.lock().await.is_empty());
}

#[tokio::test]
async fn invalid_json_types_and_oversized_body_never_reach_sleepme() {
    let fixture = Fixture::healthy().await;
    for body in [
        "{",
        "{}",
        r#"{"heart_rate":-1}"#,
        r#"{"heart_rate":65536}"#,
        r#"{"heart_rate":58.5}"#,
        r#"{"heart_rate":"58"}"#,
        r#"{"heart_rate":58,"baseline_hr":-1}"#,
    ] {
        let status = fixture
            .app
            .clone()
            .oneshot(signed(body))
            .await
            .unwrap()
            .status();
        assert!(status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY);
    }
    let response = fixture
        .app
        .clone()
        .oneshot(signed(&" ".repeat(auth::MAX_BODY_BYTES + 1)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(fixture.mock.requests.lock().await.is_empty());
}
