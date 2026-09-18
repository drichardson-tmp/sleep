# Sleep telemetry

An asynchronous Rust/Axum service that accepts signed heart-rate samples and adjusts a Sleepme bed's temperature. Tokio handles requests and a shared Reqwest client pools outbound connections. No biometric history is persisted or logged.

## Run locally

Install Rust (the repository pins 1.96.0), then configure the environment:

```sh
cp .env.example .env
# Edit .env with your credentials and a random shared secret.
# Generate a secret with: openssl rand -hex 32
set -a
source .env
set +a
cargo run --release --locked
```

The application reads process environment variables; it does not load `.env` itself.

| Variable | Required | Purpose |
| --- | --- | --- |
| `SHARED_SECRET` | Yes | HMAC key shared with the telemetry sender; use a strong random value |
| `SLEEPME_API_TOKEN` | Yes | Sleepme developer bearer token |
| `SLEEPME_DEVICE_ID` | Yes | Claimed Sleepme device ID |
| `PORT` | No | Listen on `0.0.0.0:PORT`; defaults to `8080` |
| `RUST_LOG` | No | Log filter; defaults to `info` |

Missing/empty required variables or an invalid port fail startup. SIGTERM and Ctrl-C initiate graceful shutdown.

## API

`GET /health` returns `200 OK` without authentication or external requests.

`POST /api/v1/telemetry` requires `Content-Type: application/json` and a body such as:

```json
{"heart_rate":73,"baseline_hr":58}
```

Both heart-rate fields are unsigned 16-bit integers. Omitting `baseline_hr` or setting it to `null` uses 58 BPM. The service derives a temperature offset from heart rate; the payload does not include a separate body-temperature measurement.

| Current HR minus baseline | Offset | Target from 66°F baseline |
| --- | --- | --- |
| Below 8 BPM (including negative deltas) | 0°F | 66°F |
| 8–14 BPM | −3°F | 63°F |
| 15 BPM or more | −5°F | 61°F |

Example response:

```json
{"delta_bpm":15,"offset_f":-5,"target_temperature_f":61,"updated":true}
```

`updated` is `false` when the target matches the last confirmed setpoint. On startup, the first valid sample always sets the device temperature, including a baseline sample.

### Request authentication

- `X-Timestamp`: Unix time in whole seconds, as decimal digits.
- `X-Signature`: 64 hexadecimal characters representing `HMAC-SHA256(SHARED_SECRET, timestamp + "." + raw_body)`.
- Sign and send exactly the same body bytes, including whitespace and any trailing newline.
- Timestamps must be within ±30 seconds of server time. Duplicate authentication headers are rejected.
- The digest is compared using `subtle::ConstantTimeEq::ct_eq` (the actual Rust API; `subtle::constant_time_eq` is not an exported function).

The freshness check rejects old requests but permits replay inside the window; this protocol has no nonce or persistent replay ledger. Consecutive identical targets are debounced, but replaying an earlier target after an intervening change can still change the bed. Send requests over HTTPS and keep sender clocks synchronized.

Send a sample using the included standard-library-only Python client:

```sh
python3 scripts/send_telemetry.py --url http://localhost:8080 --heart-rate 73 --baseline-hr 58
```

The script reads `SHARED_SECRET` from its environment. A successful signed request can change the configured bed's temperature.

### Sleepme integration and failures

The service sends a bearer-authenticated request to:

```http
PATCH https://api.developer.sleep.me/v1/devices/{device_id}
Content-Type: application/json

{"set_temperature_f":61}
```

Sleepme's [official examples](https://docs.developer.sleep.me/docs/apiexamples/) and [API specification](https://docs.developer.sleep.me/api/device-control.yaml) document `PATCH`, so the implementation uses it instead of the proposed POST/PUT. It only changes the temperature setpoint; enable thermal control through Sleepme when needed. The −5°F rule is a 61°F target, not Sleepme's separate MAX COLD sentinel.

The current setpoint is held in `Arc<tokio::sync::RwLock<Option<i16>>>`. One write lock covers comparison, the upstream request, and confirmation, preventing simultaneous duplicate updates and overlapping writes to the same bed. `/health` does not acquire that lock. Failures invalidate cached state, allowing the next sample to reconcile even if a timed-out request reached Sleepme. There is no automatic retry loop.

| Condition | Status |
| --- | --- |
| Missing, invalid, stale, or tampered authentication | 401 |
| Malformed JSON / wrong JSON field types | 400 / 422 |
| Wrong content type | 415 |
| Body larger than 4 KiB | 413 |
| Body takes longer than 5 seconds to arrive | 408 |
| Sleepme transport error or non-2xx response | 502 |
| Sleepme exceeds the 10-second request timeout | 504 |

Sleepme has a 3-second connection timeout. Redirects are not followed. Logs omit credentials, payloads, and upstream response bodies. This is setpoint debouncing, not a time-based rate limiter: alternating targets can still generate repeated calls. The cache is local to one process and resets after a restart; manual changes made through Sleepme are not detected. Use one replica for one bed. Rolling deployments can briefly overlap processes, so strict global deduplication would require shared coordination.

## Amazfit strap connection

The cloud API accepts heart-rate readings regardless of their source. Amazfit documents Bluetooth heart-rate broadcast for [Helio Strap](https://support.amazfit.com/us/amazfit_helio_strap/docs/GyUIdtHLvoMqNUxOkuNcQB4hn4d) and [Helio Strap Pro](https://us.amazfit.com/products/helio-strap-pro). For Helio Strap, enable **Zepp → Device → Amazfit Helio Strap → Health Monitoring → Heart Rate Push**.

Amazfit's published strap documentation does not establish support for installing custom Zepp OS mini apps on the strap. On supported Zepp OS devices, the documented [mini-app architecture](https://docs.zepp.com/docs/1.0/guides/architecture/arc/) sends HTTP requests through a side service in the phone's Zepp app. Do not assume a watch mini app can run on the strap or that Zepp automatically forwards live samples to a custom server.

This repository supplies the cloud receiver and a signed sample sender. Reading the strap over Bluetooth and forwarding fresh samples requires a phone or nearby computer integration, which is not implemented here. It need not be separate hardware if a phone provides it. Continuous overnight delivery and background behavior need testing with the purchased model; no strap hardware has been tested.

## Railway infrastructure as code

[`.railway/railway.ts`](.railway/railway.ts) uses Railway's TypeScript IaC SDK. It declares the `sleep-telemetry` service in project `sleep`, GitHub source `drichardson-tmp/sleep` on `main`, `/health`, one replica, and preserved secret variables. The root Dockerfile builds an optimized Rust binary and runs it as a non-root user. Railway supplies `PORT`.

```sh
npm --prefix .railway ci
npm --prefix .railway run check
railway link --project <your-project> --environment <your-environment>
railway config plan
# Review the plan before applying it:
railway config apply
```

Create/select the intended Railway project first. If using an existing service, name it `sleep-telemetry` or adjust the IaC service name to match before planning. Set `SHARED_SECRET`, `SLEEPME_API_TOKEN`, and `SLEEPME_DEVICE_ID` in Railway's service variables. `preserve()` retains existing values; it does not provision secret values for a new service. After a first apply creates the service, set its secrets before deploying. Never commit secrets or plan artifacts.

Push the application to the declared GitHub branch for GitHub deployment, or deploy the current checkout with `railway up --service sleep-telemetry`. Generate a Railway public domain for HTTPS access and verify `/health`. Keep the source repository/branch in IaC aligned with your intended deployment source.

The IaC describes the whole project. If adding this service to a project with unrelated resources, import that project's configuration and merge this service declaration before planning. Remote planning requires a linked project; local TypeScript validation does not replace plan review. See [Railway IaC reference](https://docs.railway.com/infrastructure-as-code/reference).

## Verification

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --locked --release
npm --prefix .railway run check
```

Tests use a local mock Sleepme server, covering authentication, freshness boundaries, JSON/body validation, exact outbound requests, temperature thresholds, concurrent debouncing, failures, redirects, and timeouts. They do not change a real bed.
