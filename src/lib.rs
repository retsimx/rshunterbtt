pub mod config;
pub mod protocol;
pub mod traits;
pub mod ble;
pub mod mqtt;
pub mod database;

#[cfg(test)]
mod app_tests;

use std::sync::Arc;
use tracing::{info, error};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use crate::config::Config;
use crate::traits::{BleClient, MqttClient, DatabaseWriter};
use crate::protocol::Second86Protocol;

#[derive(Debug, Deserialize)]
pub struct MqttCommand {
    pub cmd: String,
    pub zone: String,
    pub on_off: Option<bool>,
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct MqttResponse {
    pub cmd: String,
    pub zone: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_off: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u8>,
    pub ack: bool,
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

pub struct App {
    pub config: Config,
    pub ble_client: Arc<dyn BleClient>,
    pub mqtt_client: Arc<dyn MqttClient>,
    pub db_writer: Arc<dyn DatabaseWriter>,
}

impl App {
    pub fn new(
        config: Config,
        ble_client: Arc<dyn BleClient>,
        mqtt_client: Arc<dyn MqttClient>,
        db_writer: Arc<dyn DatabaseWriter>,
    ) -> Self {
        Self {
            config,
            ble_client,
            mqtt_client,
            db_writer,
        }
    }

    pub async fn run_command(&self, cmd: &str, zone: u8, run_time_secs: u32) -> Result<bool> {
        info!("Running command: {} for zone {} ({}s)", cmd, zone, run_time_secs);
        
        if !self.ble_client.is_connected().await {
            self.ble_client.connect(&self.config.device_address).await?;
        }

        let mut prot83 = self.ble_client.read_protocol_83().await?;
        let _status_data = self.ble_client.read_status().await?;

        match cmd {
            "start" => {
                let hours = (run_time_secs / 3600) as u8;
                let minutes = ((run_time_secs % 3600) / 60) as u8;
                let seconds = (run_time_secs % 60) as u8;

                let mut prot_zone = Second86Protocol::default();
                prot_zone.w_index = 4;
                prot_zone.zm_hour = hours;
                prot_zone.zm_minute = minutes;
                prot_zone.zm_second = seconds;

                prot83.special_setting = 0;

                if zone == 1 {
                    prot83.zone1_enable_manual = 1;
                    self.ble_client.write_protocol_86(&prot_zone).await?;
                } else if zone == 2 {
                    prot83.zone2_enable_manual = 1;
                    self.ble_client.write_protocol_8b(&prot_zone).await?;
                }
                self.ble_client.write_protocol_83(&prot83).await?;
            }
            "stop" => {
                if zone == 1 {
                    prot83.zone1_enable_manual = 0;
                } else if zone == 2 {
                    prot83.zone2_enable_manual = 0;
                }
                self.ble_client.write_protocol_83(&prot83).await?;
            }
            "status" => {
                // Status is already read
            }
            _ => return Err(anyhow!("Unknown command: {}", cmd)),
        }

        Ok(true)
    }

    pub async fn get_status(&self, zone: u8) -> Result<bool> {
        if !self.ble_client.is_connected().await {
            self.ble_client.connect(&self.config.device_address).await?;
        }
        let status_data = self.ble_client.read_status().await?;
        if zone == 1 {
            Ok(status_data.get(4).cloned().unwrap_or(0) != 0)
        } else {
            Ok(status_data.get(8).cloned().unwrap_or(0) != 0)
        }
    }

    pub async fn handle_mqtt_message(&self, payload: &[u8]) -> Result<()> {
        let msg: MqttCommand = serde_json::from_slice(payload)?;
        info!("Received MQTT command: {:?}", msg);

        let mut response = MqttResponse {
            cmd: msg.cmd.clone(),
            zone: msg.zone.clone(),
            on_off: msg.on_off,
            success: None,
            status: None,
            ack: true,
            extra: msg.extra.clone(),
        };

        let zone_id = if msg.zone == "grass" { 2 } else { 1 };

        match msg.cmd.as_str() {
            "on_off" => {
                if let Some(on) = msg.on_off {
                    let success = if on {
                        self.run_command("start", zone_id, 2 * 3600).await.is_ok()
                    } else {
                        self.run_command("stop", zone_id, 0).await.is_ok()
                    };
                    response.success = Some(success);
                }
            }
            "status" => {
                let status = self.get_status(zone_id).await?;
                response.status = Some(if status { 1 } else { 0 });
            }
            _ => {
                error!("Unknown command: {}", msg.cmd);
                return Ok(());
            }
        }

        let resp_payload = serde_json::to_string(&response)?;
        info!("Publishing response to {}: {}", self.config.mqtt_pub_topic, resp_payload);
        self.mqtt_client.publish(&self.config.mqtt_pub_topic, &resp_payload).await?;

        Ok(())
    }

    pub async fn poll_battery(&self) -> Result<()> {
        info!("Polling battery level for {}...", self.config.device_name);
        if !self.ble_client.is_connected().await {
            info!("BLE not connected, connecting to {}...", self.config.device_address);
            self.ble_client.connect(&self.config.device_address).await?;
        }
        let level = self.ble_client.read_battery().await?;
        info!("Battery level: {}%", level);
        self.db_writer.write_battery(&self.config.device_name, level).await?;
        info!("Battery level successfully written to database.");
        Ok(())
    }
}

pub async fn run_app() -> Result<()> {
    tracing_subscriber::fmt::init();
    
    loop {
        info!("Starting rshunterbtt...");
        if let Err(e) = run_app_instance().await {
            error!("Application instance crashed: {}. Restarting in 10s...", e);
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
    }
}

async fn run_app_instance() -> Result<()> {
    let config = Config::from_env()?;
    
    let ble_client = Arc::new(crate::ble::BtleplugClient::new().await?);
    
    let (mqtt_client_impl, mut eventloop) = crate::mqtt::RumqttcClient::new(
        &config.mqtt_broker,
        config.mqtt_port,
        &format!("rshunterbtt-{}", config.device_name)
    );
    let mqtt_client = Arc::new(mqtt_client_impl);
    
    let db_writer = Arc::new(crate::database::InfluxDbWriter::new(
        &config.influxdb_url,
        &config.influxdb_token,
        &config.influxdb_org,
        &config.influxdb_bucket
    ));

    let app = Arc::new(App::new(config.clone(), ble_client, mqtt_client.clone(), db_writer));

    // Start battery polling task
    let app_clone = app.clone();
    tokio::spawn(async move {
        loop {
            match app_clone.poll_battery().await {
                Ok(_) => {
                    tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                }
                Err(e) => {
                    error!("Battery poll failed: {}. Retrying in 60s...", e);
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                }
            }
        }
    });

    // Subscribe to MQTT topic
    info!("Subscribing to MQTT topic: {}", config.mqtt_sub_topic);
    mqtt_client.subscribe(&config.mqtt_sub_topic).await?;
    info!("Successfully subscribed.");

    // MQTT Event Loop
    loop {
        match eventloop.poll().await {
            Ok(notification) => {
                if let rumqttc::Event::Incoming(rumqttc::Packet::Publish(publish)) = notification {
                    info!("Incoming MQTT publish on topic: {}", publish.topic);
                    let app_clone = app.clone();
                    tokio::spawn(async move {
                        if let Err(e) = app_clone.handle_mqtt_message(&publish.payload).await {
                            error!("Error handling MQTT message: {}", e);
                        }
                    });
                }
            }
            Err(e) => {
                error!("MQTT loop error: {}. Returning to supervisor for restart...", e);
                return Err(anyhow!("MQTT EventLoop error: {}", e));
            }
        }
    }
}
