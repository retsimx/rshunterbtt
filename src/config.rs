use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub device_address: String,
    pub device_name: String,
    pub mqtt_broker: String,
    pub mqtt_port: u16,
    pub mqtt_sub_topic: String,
    pub mqtt_pub_topic: String,
    pub influxdb_url: String,
    pub influxdb_token: String,
    pub influxdb_org: String,
    pub influxdb_bucket: String,
    pub device_password: Option<String>,
    pub default_run_seconds: u32,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        dotenvy::dotenv().ok();

        let config = Config {
            device_address: std::env::var("DEVICE_ADDRESS").context("DEVICE_ADDRESS not set")?,
            device_name: std::env::var("DEVICE_NAME").context("DEVICE_NAME not set")?,
            mqtt_broker: std::env::var("MQTT_BROKER").context("MQTT_BROKER not set")?,
            mqtt_port: std::env::var("MQTT_PORT")
                .unwrap_or_else(|_| "1883".to_string())
                .parse()
                .context("MQTT_PORT must be a number")?,
            mqtt_sub_topic: std::env::var("MQTT_SUB_TOPIC").context("MQTT_SUB_TOPIC not set")?,
            mqtt_pub_topic: std::env::var("MQTT_PUB_TOPIC").context("MQTT_PUB_TOPIC not set")?,
            influxdb_url: std::env::var("INFLUXDB_URL").context("INFLUXDB_URL not set")?,
            influxdb_token: std::env::var("INFLUXDB_TOKEN").context("INFLUXDB_TOKEN not set")?,
            influxdb_org: std::env::var("INFLUXDB_ORG").context("INFLUXDB_ORG not set")?,
            influxdb_bucket: std::env::var("INFLUXDB_BUCKET").context("INFLUXDB_BUCKET not set")?,
            device_password: std::env::var("DEVICE_PASSWORD").ok(),
            default_run_seconds: std::env::var("DEFAULT_RUN_SECONDS")
                .unwrap_or_else(|_| "7200".to_string())
                .parse()
                .context("DEFAULT_RUN_SECONDS must be a number")?,
        };

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn required_env() {
        std::env::set_var("DEVICE_ADDRESS", "11:22:33:44:55:66");
        std::env::set_var("DEVICE_NAME", "test_device");
        std::env::set_var("MQTT_BROKER", "localhost");
        std::env::set_var("MQTT_PORT", "1883");
        std::env::set_var("MQTT_SUB_TOPIC", "sub");
        std::env::set_var("MQTT_PUB_TOPIC", "pub");
        std::env::set_var("INFLUXDB_URL", "http://localhost:8086");
        std::env::set_var("INFLUXDB_TOKEN", "token");
        std::env::set_var("INFLUXDB_ORG", "org");
        std::env::set_var("INFLUXDB_BUCKET", "bucket");
    }

    #[test]
    fn default_run_seconds_defaults_to_7200_when_unset() {
        let _guard = ENV_LOCK.lock().unwrap();
        required_env();
        std::env::remove_var("DEFAULT_RUN_SECONDS");
        let config = Config::from_env().unwrap();
        assert_eq!(config.default_run_seconds, 7200);
    }

    #[test]
    fn default_run_seconds_parses_when_set() {
        let _guard = ENV_LOCK.lock().unwrap();
        required_env();
        std::env::set_var("DEFAULT_RUN_SECONDS", "1800");
        let config = Config::from_env().unwrap();
        assert_eq!(config.default_run_seconds, 1800);
    }

    #[test]
    fn default_run_seconds_fails_when_invalid() {
        let _guard = ENV_LOCK.lock().unwrap();
        required_env();
        std::env::set_var("DEFAULT_RUN_SECONDS", "not_a_number");
        assert!(Config::from_env().is_err());
    }
}
