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
        };

        Ok(config)
    }
}
