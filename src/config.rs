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
    pub conn_interval_ms: u16,
    pub ha_status_topic: Option<String>,
    pub ha_birth_payload: String,
    pub state_heartbeat_secs: u64,
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
            conn_interval_ms: {
                let ms: u16 = std::env::var("CONN_INTERVAL_MS")
                    .unwrap_or_else(|_| "4000".to_string())
                    .parse()
                    .context("CONN_INTERVAL_MS must be a number")?;
                if !(8..=4000).contains(&ms) {
                    return Err(anyhow::anyhow!(
                        "CONN_INTERVAL_MS must be between 8 and 4000 (BLE range), got {}",
                        ms
                    ));
                }
                ms
            },
            ha_status_topic: {
                let raw = std::env::var("HA_STATUS_TOPIC")
                    .unwrap_or_else(|_| "homeassistant/status".to_string());
                if raw.is_empty() {
                    None
                } else {
                    Some(raw)
                }
            },
            ha_birth_payload: std::env::var("HA_BIRTH_PAYLOAD")
                .unwrap_or_else(|_| "online".to_string()),
            state_heartbeat_secs: std::env::var("STATE_HEARTBEAT_SECS")
                .unwrap_or_else(|_| "60".to_string())
                .parse()
                .context("STATE_HEARTBEAT_SECS must be a number")?,
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
        // Optional vars: start each test from a known state.
        std::env::remove_var("CONN_INTERVAL_MS");
        std::env::remove_var("DEFAULT_RUN_SECONDS");
        std::env::remove_var("HA_STATUS_TOPIC");
        std::env::remove_var("HA_BIRTH_PAYLOAD");
        std::env::remove_var("STATE_HEARTBEAT_SECS");
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

    #[test]
    fn conn_interval_defaults_to_4000_when_unset() {
        let _guard = ENV_LOCK.lock().unwrap();
        required_env();
        std::env::remove_var("CONN_INTERVAL_MS");
        let config = Config::from_env().unwrap();
        assert_eq!(config.conn_interval_ms, 4000);
    }

    #[test]
    fn conn_interval_parses_when_set() {
        let _guard = ENV_LOCK.lock().unwrap();
        required_env();
        std::env::set_var("CONN_INTERVAL_MS", "1000");
        let config = Config::from_env().unwrap();
        assert_eq!(config.conn_interval_ms, 1000);
    }

    #[test]
    fn conn_interval_fails_when_out_of_ble_range() {
        let _guard = ENV_LOCK.lock().unwrap();
        required_env();
        std::env::set_var("CONN_INTERVAL_MS", "9999");
        assert!(Config::from_env().is_err());
        std::env::set_var("CONN_INTERVAL_MS", "1");
        assert!(Config::from_env().is_err());
    }

    #[test]
    fn ha_status_topic_defaults_to_homeassistant_status() {
        let _guard = ENV_LOCK.lock().unwrap();
        required_env();
        let config = Config::from_env().unwrap();
        assert_eq!(
            config.ha_status_topic.as_deref(),
            Some("homeassistant/status")
        );
    }

    #[test]
    fn ha_status_topic_parses_override() {
        let _guard = ENV_LOCK.lock().unwrap();
        required_env();
        std::env::set_var("HA_STATUS_TOPIC", "custom/status");
        let config = Config::from_env().unwrap();
        assert_eq!(config.ha_status_topic.as_deref(), Some("custom/status"));
    }

    #[test]
    fn ha_status_topic_empty_disables() {
        let _guard = ENV_LOCK.lock().unwrap();
        required_env();
        std::env::set_var("HA_STATUS_TOPIC", "");
        let config = Config::from_env().unwrap();
        assert_eq!(config.ha_status_topic, None);
    }

    #[test]
    fn ha_birth_payload_defaults_to_online() {
        let _guard = ENV_LOCK.lock().unwrap();
        required_env();
        let config = Config::from_env().unwrap();
        assert_eq!(config.ha_birth_payload, "online");
    }

    #[test]
    fn ha_birth_payload_parses_override() {
        let _guard = ENV_LOCK.lock().unwrap();
        required_env();
        std::env::set_var("HA_BIRTH_PAYLOAD", "ready");
        let config = Config::from_env().unwrap();
        assert_eq!(config.ha_birth_payload, "ready");
    }

    #[test]
    fn state_heartbeat_defaults_to_60_when_unset() {
        let _guard = ENV_LOCK.lock().unwrap();
        required_env();
        let config = Config::from_env().unwrap();
        assert_eq!(config.state_heartbeat_secs, 60);
    }

    #[test]
    fn state_heartbeat_parses_when_set() {
        let _guard = ENV_LOCK.lock().unwrap();
        required_env();
        std::env::set_var("STATE_HEARTBEAT_SECS", "15");
        let config = Config::from_env().unwrap();
        assert_eq!(config.state_heartbeat_secs, 15);
    }

    #[test]
    fn state_heartbeat_zero_disables() {
        let _guard = ENV_LOCK.lock().unwrap();
        required_env();
        std::env::set_var("STATE_HEARTBEAT_SECS", "0");
        let config = Config::from_env().unwrap();
        assert_eq!(config.state_heartbeat_secs, 0);
    }

    #[test]
    fn state_heartbeat_fails_when_invalid() {
        let _guard = ENV_LOCK.lock().unwrap();
        required_env();
        std::env::set_var("STATE_HEARTBEAT_SECS", "not_a_number");
        assert!(Config::from_env().is_err());
    }
}
