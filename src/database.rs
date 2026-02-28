use async_trait::async_trait;
use anyhow::Result;
use influxdb::{Client, WriteQuery};
use crate::traits::DatabaseWriter;

pub struct InfluxDbWriter {
    client: Client,
}

impl InfluxDbWriter {
    pub fn new(url: &str, token: &str, _org: &str, bucket: &str) -> Self {
        let client = Client::new(url, bucket).with_token(token);
        Self {
            client,
        }
    }
}

#[async_trait]
impl DatabaseWriter for InfluxDbWriter {
    async fn write_battery(&self, device_name: &str, level: u8) -> Result<()> {
        let query = WriteQuery::new(chrono::Utc::now().into(), "battery")
            .add_tag("name", device_name)
            .add_field("battery", level as i64);
        
        self.client.query(query).await.map_err(|e| anyhow::anyhow!(e))?;
        Ok(())
    }
}
