use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::{
    body::{Body, to_bytes},
    extract::{Request, State},
    http::HeaderMap,
    middleware::Next,
    response::Response,
};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use crate::{ApiError, AppState};

pub const MAX_BODY_BYTES: usize = 4096;
const FRESHNESS_SECONDS: u64 = 30;

pub async fn authenticate(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let (parts, body) = request.into_parts();
    let timestamp = single_header(&parts.headers, "x-timestamp")?;
    let signature = single_header(&parts.headers, "x-signature")?;
    let timestamp_seconds = parse_timestamp(timestamp)?;
    check_freshness(timestamp_seconds, now()?)?;

    let mut supplied = [0_u8; 32];
    hex::decode_to_slice(signature, &mut supplied).map_err(|_| ApiError::Unauthorized)?;
    let body = tokio::time::timeout(Duration::from_secs(5), to_bytes(body, MAX_BODY_BYTES))
        .await
        .map_err(|_| ApiError::BodyTimeout)?
        .map_err(|error| {
            use std::error::Error;
            // Inspect the chain because Axum wraps the body limit error.
            let mut source: &(dyn Error + 'static) = &error;
            loop {
                if source.is::<http_body_util::LengthLimitError>() {
                    break ApiError::BodyTooLarge;
                }
                match source.source() {
                    Some(next) => source = next,
                    None => break ApiError::BadBody,
                }
            }
        })?;

    // Sign the exact wire bytes, including whitespace, without reserializing JSON.
    let mut mac = Hmac::<Sha256>::new_from_slice(&state.shared_secret)
        .expect("HMAC accepts keys of any length");
    mac.update(timestamp.as_bytes());
    mac.update(b".");
    mac.update(&body);
    let expected = mac.finalize().into_bytes();
    // subtle exposes ConstantTimeEq::ct_eq, not a constant_time_eq free function.
    if !bool::from(expected.as_slice().ct_eq(&supplied)) {
        return Err(ApiError::Unauthorized);
    }
    check_freshness(timestamp_seconds, now()?)?;

    Ok(next.run(Request::from_parts(parts, Body::from(body))).await)
}

fn single_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, ApiError> {
    let mut values = headers.get_all(name).iter();
    let value = values.next().ok_or(ApiError::Unauthorized)?;
    if values.next().is_some() {
        return Err(ApiError::Unauthorized);
    }
    value.to_str().map_err(|_| ApiError::Unauthorized)
}

fn parse_timestamp(timestamp: &str) -> Result<u64, ApiError> {
    if timestamp.is_empty()
        || timestamp.len() > 20
        || !timestamp.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(ApiError::Unauthorized);
    }
    timestamp.parse().map_err(|_| ApiError::Unauthorized)
}

fn now() -> Result<u64, ApiError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| ApiError::Internal)
}

fn check_freshness(timestamp: u64, now: u64) -> Result<(), ApiError> {
    if now.abs_diff(timestamp) > FRESHNESS_SECONDS {
        return Err(ApiError::Unauthorized);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freshness_is_inclusive_and_rejects_both_directions() {
        for timestamp in [970, 1000, 1030] {
            assert!(check_freshness(timestamp, 1000).is_ok());
        }
        for timestamp in [0, 969, 1031, u64::MAX] {
            assert!(check_freshness(timestamp, 1000).is_err());
        }
    }
}
