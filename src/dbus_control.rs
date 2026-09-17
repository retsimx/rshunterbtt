use crate::traits::ResilienceController;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use dbus::blocking::stdintf::org_freedesktop_dbus::Properties;
use dbus::blocking::Connection;
use std::time::Duration;

const DBUS_BUS: &str = "org.bluez";
const ADAPTER_INTERFACE: &str = "org.bluez.Adapter1";
const PROPERTY_POWERED: &str = "Powered";
const POWER_CYCLE_DELAY: Duration = Duration::from_secs(1);

pub struct SystemResilienceController {
    adapter_path: String,
}

impl SystemResilienceController {
    pub fn new(adapter_path: impl Into<String>) -> Self {
        Self {
            adapter_path: adapter_path.into(),
        }
    }
}

impl Default for SystemResilienceController {
    fn default() -> Self {
        Self::new("/org/bluez/hci0")
    }
}

#[async_trait]
impl ResilienceController for SystemResilienceController {
    async fn power_cycle_adapter(&self) -> Result<()> {
        let conn = Connection::new_system().map_err(|e| anyhow!("D-Bus system bus: {}", e))?;
        let proxy = conn.with_proxy(DBUS_BUS, &self.adapter_path, Duration::from_secs(5));

        proxy
            .set(ADAPTER_INTERFACE, PROPERTY_POWERED, false)
            .map_err(|e| anyhow!("set Powered=false: {}", e))?;

        std::thread::sleep(POWER_CYCLE_DELAY);

        proxy
            .set(ADAPTER_INTERFACE, PROPERTY_POWERED, true)
            .map_err(|e| anyhow!("set Powered=true: {}", e))?;

        Ok(())
    }

    async fn reboot_host(&self) -> Result<()> {
        crate::reboot::reboot_host()
    }
}
