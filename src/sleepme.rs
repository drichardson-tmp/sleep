use std::{error::Error, time::Duration};

use reqwest::{
    Client, Url,
    header::{AUTHORIZATION, HeaderMap, HeaderValue},
};
use serde::Serialize;

use crate::ApiError;

#[derive(Clone)]
pub struct SleepmeClient {
    client: Client,
    device_url: Url,
}

impl SleepmeClient {
    pub fn new(token: &str, device_id: &str) -> Result<Self, Box<dyn Error>> {
        if device_id.trim().is_empty() || device_id == "." || device_id == ".." {
            return Err("SLEEPME_DEVICE_ID must be a nonempty device identifier".into());
        }
        let mut device_url = Url::parse("https://api.developer.sleep.me/v1/devices/")?;
        device_url
            .path_segments_mut()
            .expect("HTTPS URL has path segments")
            .pop_if_empty()
            .push(device_id);
        Self::with_url(token, device_url, Duration::from_secs(10))
    }

    fn with_url(token: &str, device_url: Url, timeout: Duration) -> Result<Self, Box<dyn Error>> {
        let mut authorization = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| "SLEEPME_API_TOKEN contains invalid header characters")?;
        authorization.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, authorization);
        let client = Client::builder()
            .default_headers(headers)
            .connect_timeout(Duration::from_secs(3))
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self { client, device_url })
    }

    pub(crate) async fn set_temperature(&self, target_f: i16) -> Result<(), ApiError> {
        #[derive(Serialize)]
        struct Update {
            set_temperature_f: i16,
        }

        // Sleepme's documented device update method is PATCH with a flat body.
        let response = self
            .client
            .patch(self.device_url.clone())
            .json(&Update {
                set_temperature_f: target_f,
            })
            .send()
            .await
            .map_err(|error| {
                tracing::warn!(timeout = error.is_timeout(), "Sleepme request failed");
                if error.is_timeout() {
                    ApiError::UpstreamTimeout
                } else {
                    ApiError::Upstream
                }
            })?;
        if !response.status().is_success() {
            tracing::warn!(
                status = response.status().as_u16(),
                "Sleepme rejected update"
            );
            return Err(ApiError::Upstream);
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn mock(url: Url, timeout: Duration) -> Self {
        Self::with_url("test-token", url, timeout).unwrap()
    }
}
