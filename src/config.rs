use std::{env, error::Error};

pub struct Config {
    pub shared_secret: String,
    pub sleepme_api_token: String,
    pub sleepme_device_id: String,
    pub port: u16,
}

impl Config {
    pub fn from_env() -> Result<Self, Box<dyn Error>> {
        let port = match env::var("PORT") {
            Ok(value) => value
                .parse::<u16>()
                .map_err(|_| "PORT must be an integer from 1 to 65535")?,
            Err(env::VarError::NotPresent) => 8080,
            Err(_) => return Err("PORT must be valid Unicode".into()),
        };
        if port == 0 {
            return Err("PORT must be an integer from 1 to 65535".into());
        }
        Ok(Self {
            shared_secret: required("SHARED_SECRET")?,
            sleepme_api_token: required("SLEEPME_API_TOKEN")?,
            sleepme_device_id: required("SLEEPME_DEVICE_ID")?,
            port,
        })
    }
}

fn required(name: &str) -> Result<String, Box<dyn Error>> {
    let value = env::var(name).map_err(|_| format!("{name} is required"))?;
    if value.trim().is_empty() {
        return Err(format!("{name} must not be empty").into());
    }
    Ok(value)
}
