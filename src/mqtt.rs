use anyhow::{Result, anyhow};
use async_trait::async_trait;
use rumqttc::{AsyncClient, MqttOptions, QoS};
use crate::traits::MqttClient;

pub struct RumqttcClient {
    client: AsyncClient,
}

impl RumqttcClient {
    pub fn new(host: &str, port: u16, client_id: &str) -> (Self, rumqttc::EventLoop) {
        let mut mqttoptions = MqttOptions::new(client_id, host, port);
        mqttoptions.set_keep_alive(std::time::Duration::from_secs(5));

        let (client, eventloop) = AsyncClient::new(mqttoptions, 10);
        (Self { client }, eventloop)
    }
}

#[async_trait]
impl MqttClient for RumqttcClient {
    async fn publish(&self, topic: &str, payload: &str) -> Result<()> {
        self.client.publish(topic, QoS::AtLeastOnce, false, payload.as_bytes()).await.map_err(|e| anyhow!(e))
    }

    async fn subscribe(&self, topic: &str) -> Result<()> {
        self.client.subscribe(topic, QoS::AtLeastOnce).await.map_err(|e| anyhow!(e))
    }
}
