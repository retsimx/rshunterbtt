use anyhow::{anyhow, Result};
use async_trait::async_trait;
use btleplug::api::{
    Central, Characteristic, Manager as _, Peripheral as _, ScanFilter, WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use futures::StreamExt;
use std::time::Duration;
use tokio::time;
use tracing::{debug, info};
use uuid::Uuid;

use crate::protocol::{
    decode_zone_name, protocol_id_to_uuid, Second83Protocol, Second86Protocol,
    BATTERY_LEVEL_CHAR_UUID,
};
use crate::traits::BleClient;

pub struct BtleplugClient {
    central: Adapter,
    peripheral: tokio::sync::Mutex<Option<Peripheral>>,
}

impl BtleplugClient {
    pub async fn new() -> Result<Self> {
        let manager = Manager::new().await?;
        let adapters = manager.adapters().await?;
        let central = adapters
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("No adapters found"))?;
        Ok(Self {
            central,
            peripheral: tokio::sync::Mutex::new(None),
        })
    }

    async fn get_peripheral(&self) -> Result<Peripheral> {
        self.peripheral
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow!("Not connected"))
    }

    async fn find_characteristic(
        &self,
        peripheral: &Peripheral,
        uuid_str: &str,
    ) -> Result<Characteristic> {
        let target_uuid = Uuid::parse_str(uuid_str)?;
        for service in peripheral.services() {
            for char in service.characteristics {
                if char.uuid == target_uuid {
                    return Ok(char);
                }
            }
        }
        Err(anyhow!("Characteristic {} not found", uuid_str))
    }
}

#[async_trait]
impl BleClient for BtleplugClient {
    async fn connect(&self, address: &str) -> Result<()> {
        info!("Starting BLE scan for {}...", address);
        match self.central.start_scan(ScanFilter::default()).await {
            Ok(_) => debug!("Scan started successfully"),
            Err(e) => {
                let err_msg = format!("{:?}", e);
                if err_msg.contains("AlreadyInProgress")
                    || err_msg.contains("already in progress")
                    || err_msg.contains("Operation already in progress")
                {
                    info!(
                        "BLE scan already in progress (matched: {}), continuing to search...",
                        err_msg
                    );
                } else {
                    return Err(anyhow!("Scan error: {}", err_msg));
                }
            }
        }

        time::sleep(Duration::from_secs(10)).await;

        let peripherals = self.central.peripherals().await?;
        for p in peripherals {
            if p.address().to_string() == address {
                info!("Found device {}, connecting...", address);

                // Try to stop scanning before connecting if we were the ones who started it
                let _ = self.central.stop_scan().await;

                p.connect().await?;
                info!("Connected to {}. Discovering services...", address);
                p.discover_services().await?;
                info!("Services discovered for {}.", address);
                let mut lock = self.peripheral.lock().await;
                *lock = Some(p);
                return Ok(());
            }
        }

        Err(anyhow!("Device {} not found", address))
    }

    async fn is_connected(&self) -> bool {
        if let Some(p) = self.peripheral.lock().await.as_ref() {
            p.is_connected().await.unwrap_or(false)
        } else {
            false
        }
    }

    async fn read_protocol_83(&self) -> Result<Second83Protocol> {
        let p = self.get_peripheral().await?;
        let char = self
            .find_characteristic(&p, &protocol_id_to_uuid(65411))
            .await?;
        let data = p.read(&char).await?;
        debug!("Read protocol 83: {}", hex::encode(&data));
        Second83Protocol::from_bytes(&data)
    }

    async fn read_status(&self) -> Result<Vec<u8>> {
        let p = self.get_peripheral().await?;
        let char = self
            .find_characteristic(&p, &protocol_id_to_uuid(65410))
            .await?;
        let data = p.read(&char).await?;
        debug!("Read status: {}", hex::encode(&data));
        Ok(data)
    }

    async fn write_protocol_83(&self, data: &Second83Protocol) -> Result<()> {
        let p = self.get_peripheral().await?;
        let char = self
            .find_characteristic(&p, &protocol_id_to_uuid(65411))
            .await?;
        let bytes = data.to_bytes();
        debug!("Writing protocol 83: {}", hex::encode(&bytes));
        p.write(&char, &bytes, WriteType::WithResponse)
            .await
            .map_err(|e| anyhow!(e))
    }

    async fn write_protocol_86(&self, data: &Second86Protocol) -> Result<()> {
        let p = self.get_peripheral().await?;
        let char = self
            .find_characteristic(&p, &protocol_id_to_uuid(65414))
            .await?;
        let bytes = data.to_bytes();
        debug!("Writing protocol 86: {}", hex::encode(&bytes));
        p.write(&char, &bytes, WriteType::WithResponse)
            .await
            .map_err(|e| anyhow!(e))
    }

    async fn write_protocol_8b(&self, data: &Second86Protocol) -> Result<()> {
        let p = self.get_peripheral().await?;
        let char = self
            .find_characteristic(&p, &protocol_id_to_uuid(65419))
            .await?;
        let bytes = data.to_bytes();
        debug!("Writing protocol 8b: {}", hex::encode(&bytes));
        p.write(&char, &bytes, WriteType::WithResponse)
            .await
            .map_err(|e| anyhow!(e))
    }

    async fn read_battery(&self) -> Result<u8> {
        let p = self.get_peripheral().await?;
        let char = self
            .find_characteristic(&p, BATTERY_LEVEL_CHAR_UUID)
            .await?;
        let data = p.read(&char).await?;
        debug!("Read battery: {}", hex::encode(&data));
        data.first()
            .cloned()
            .ok_or_else(|| anyhow!("Battery data empty"))
    }

    async fn read_zone1_name(&self) -> Result<String> {
        let p = self.get_peripheral().await?;
        let char = self
            .find_characteristic(&p, "0000ff90-0000-1000-8000-00805f9b34fb")
            .await?;
        let data = p.read(&char).await?;
        debug!("Read zone 1 name: {}", hex::encode(&data));
        decode_zone_name(&data)
    }

    async fn read_zone2_name(&self) -> Result<String> {
        let p = self.get_peripheral().await?;
        let char = self
            .find_characteristic(&p, "0000ff91-0000-1000-8000-00805f9b34fb")
            .await?;
        let data = p.read(&char).await?;
        debug!("Read zone 2 name: {}", hex::encode(&data));
        decode_zone_name(&data)
    }

    async fn write_password(&self, password: &[u8; 4]) -> Result<()> {
        let p = self.get_peripheral().await?;
        let char = self
            .find_characteristic(&p, "0000ff81-0000-1000-8000-00805f9b34fb")
            .await?;
        debug!("Writing password to ff81");
        p.write(&char, password, WriteType::WithResponse)
            .await
            .map_err(|e| anyhow!(e))
    }

    async fn subscribe_notifications(
        &self,
        uuid_str: &str,
    ) -> Result<tokio::sync::mpsc::Receiver<Vec<u8>>> {
        let p = self.get_peripheral().await?;
        let char = self.find_characteristic(&p, uuid_str).await?;
        p.subscribe(&char).await?;
        let mut stream = p.notifications().await?;
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let target = char.uuid;
        tokio::spawn(async move {
            while let Some(notif) = stream.next().await {
                if notif.uuid == target && tx.send(notif.value).await.is_err() {
                    break;
                }
            }
        });
        Ok(rx)
    }
}
