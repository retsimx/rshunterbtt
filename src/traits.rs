use crate::protocol::{Second83Protocol, Second86Protocol};
use anyhow::Result;
use async_trait::async_trait;

#[cfg(any(test, feature = "mockall"))]
use mockall::automock;

#[async_trait]
#[cfg_attr(any(test, feature = "mockall"), automock)]
pub trait BleClient: Send + Sync {
    async fn connect(&self, address: &str) -> Result<()>;
    async fn disconnect(&self) -> Result<()>;
    async fn is_connected(&self) -> bool;
    async fn read_protocol_83(&self) -> Result<Second83Protocol>;
    async fn read_status(&self) -> Result<Vec<u8>>;
    async fn write_protocol_83(&self, data: &Second83Protocol) -> Result<()>;
    async fn write_protocol_86(&self, data: &Second86Protocol) -> Result<()>;
    async fn write_protocol_8b(&self, data: &Second86Protocol) -> Result<()>;
    async fn read_battery(&self) -> Result<u8>;
    async fn read_zone1_name(&self) -> Result<String>;
    async fn read_zone2_name(&self) -> Result<String>;
    async fn write_password(&self, password: &[u8; 4]) -> Result<()>;
    async fn subscribe_notifications(
        &self,
        uuid_str: &str,
    ) -> Result<tokio::sync::mpsc::Receiver<Vec<u8>>>;
}

#[async_trait]
#[cfg_attr(any(test, feature = "mockall"), automock)]
pub trait MqttClient: Send + Sync {
    async fn publish(&self, topic: &str, payload: &str) -> Result<()>;
    async fn subscribe(&self, topic: &str) -> Result<()>;
}

#[async_trait]
#[cfg_attr(any(test, feature = "mockall"), automock)]
pub trait DatabaseWriter: Send + Sync {
    async fn write_battery(&self, device_name: &str, level: u8) -> Result<()>;
    async fn write_valve_event(&self, device_name: &str, zone: &str, state: bool) -> Result<()>;
}

#[async_trait]
#[cfg_attr(any(test, feature = "mockall"), automock)]
pub trait ResilienceController: Send + Sync {
    async fn power_cycle_adapter(&self) -> Result<()>;
    async fn reboot_host(&self) -> Result<()>;
}
