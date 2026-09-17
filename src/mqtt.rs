use crate::traits::MqttClient;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use rumqttc::{AsyncClient, MqttOptions, QoS};

pub struct RumqttcClient {
    client: AsyncClient,
}

impl RumqttcClient {
    pub fn new(host: &str, port: u16, client_id: &str) -> (Self, rumqttc::EventLoop) {
        let mut mqttoptions = MqttOptions::new(client_id, host, port);
        // 60s keepalive (broker tolerates 1.5x = 90s). A short keepalive (the
        // previous 5s) is missed while the single-core Pi Zero W is busy with a
        // BLE connect/scan, so the broker closes the connection and the bridge
        // enters a crash loop.
        mqttoptions.set_keep_alive(std::time::Duration::from_secs(60));

        let (client, eventloop) = AsyncClient::new(mqttoptions, 10);
        (Self { client }, eventloop)
    }
}

#[async_trait]
impl MqttClient for RumqttcClient {
    async fn publish(&self, topic: &str, payload: &str) -> Result<()> {
        self.client
            .publish(topic, QoS::AtLeastOnce, false, payload.as_bytes())
            .await
            .map_err(|e| anyhow!(e))
    }

    async fn subscribe(&self, topic: &str) -> Result<()> {
        self.client
            .subscribe(topic, QoS::AtLeastOnce)
            .await
            .map_err(|e| anyhow!(e))
    }
}
