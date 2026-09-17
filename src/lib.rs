pub mod ble;
pub mod config;
pub mod database;
pub mod dbus_control;
pub mod hci;
pub mod hci_monitor;
pub mod mqtt;
pub mod protocol;
pub mod reboot;
pub mod resilience;
pub mod traits;

#[cfg(test)]
mod app_tests;

use crate::config::Config;
use crate::protocol::{Second82Protocol, Second86Protocol};
use crate::resilience::{LadderAction, ResilienceLadder, ResilienceState, ResilienceStateStore};
use crate::traits::{BleClient, DatabaseWriter, MqttClient, ResilienceController};
use anyhow::{anyhow, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

pub(crate) type StatusCache = watch::Receiver<Option<Second82Protocol>>;
pub(crate) type StatusCacheSender = watch::Sender<Option<Second82Protocol>>;
pub(crate) type ZoneNamesCache = watch::Receiver<Option<ZoneNames>>;
pub(crate) type ZoneNamesCacheSender = watch::Sender<Option<ZoneNames>>;

#[derive(Debug, Clone, Default)]
pub struct ZoneNames {
    pub zone1: Option<String>,
    pub zone2: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MqttCommand {
    pub cmd: String,
    pub zone: String,
    pub on_off: Option<bool>,
    pub duration_seconds: Option<u32>,
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
    connection_ready: watch::Receiver<bool>,
    status_cache: StatusCache,
    status_tx: StatusCacheSender,
    zone_names: ZoneNamesCache,
    /// Serializes MQTT-triggered BLE work. The MQTT loop spawns a task per
    /// message, so without this two commands can interleave GATT operations on
    /// the same device and time out ("Timeout waiting for reply").
    command_lock: tokio::sync::Mutex<()>,
}

impl App {
    pub fn new(
        config: Config,
        ble_client: Arc<dyn BleClient>,
        mqtt_client: Arc<dyn MqttClient>,
        db_writer: Arc<dyn DatabaseWriter>,
    ) -> Self {
        let (_, ready_rx) = watch::channel(true);
        let (status_tx, status_rx) = watch::channel(None);
        let (_, zone_names_rx) = watch::channel(None);
        Self {
            config,
            ble_client,
            mqtt_client,
            db_writer,
            connection_ready: ready_rx,
            status_cache: status_rx,
            status_tx,
            zone_names: zone_names_rx,
            command_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub fn with_connection_ready(mut self, connection_ready: watch::Receiver<bool>) -> Self {
        self.connection_ready = connection_ready;
        self
    }

    pub fn with_status_cache(
        mut self,
        status_cache: StatusCache,
        status_tx: StatusCacheSender,
    ) -> Self {
        self.status_cache = status_cache;
        self.status_tx = status_tx;
        self
    }

    pub fn with_zone_names(mut self, zone_names: ZoneNamesCache) -> Self {
        self.zone_names = zone_names;
        self
    }

    async fn ensure_connection_ready(&self) -> Result<()> {
        if *self.connection_ready.borrow() {
            return Ok(());
        }
        let mut rx = self.connection_ready.clone();
        let deadline = tokio::time::Instant::now() + CONNECTION_READY_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(anyhow!(
                    "Timed out waiting for BLE connection to become ready"
                ));
            }
            tokio::select! {
                _ = rx.changed() => {
                    if *rx.borrow() {
                        return Ok(());
                    }
                }
                _ = tokio::time::sleep(remaining) => {
                    return Err(anyhow!("Timed out waiting for BLE connection to become ready"));
                }
            }
        }
    }

    pub async fn run_command(&self, cmd: &str, zone: u8, run_time_secs: u32) -> Result<bool> {
        info!(
            "Running command: {} for zone {} ({}s)",
            cmd, zone, run_time_secs
        );

        self.ensure_connection_ready().await?;

        let mut prot83 = self.ble_client.read_protocol_83().await?;
        let _status_data = self.ble_client.read_status().await?;

        match cmd {
            "start" => {
                if run_time_secs / 3600 > u8::MAX as u32 {
                    return Ok(false);
                }
                let hours = (run_time_secs / 3600) as u8;
                let minutes = ((run_time_secs % 3600) / 60) as u8;
                let seconds = (run_time_secs % 60) as u8;

                let prot_zone = Second86Protocol {
                    w_index: 4,
                    zm_hour: hours,
                    zm_minute: minutes,
                    zm_second: seconds,
                    ..Default::default()
                };

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
        self.ensure_connection_ready().await?;
        let cached = self.status_cache.borrow().clone();
        let protocol = match cached {
            Some(p) => p,
            None => {
                let status_data = self.ble_client.read_status().await?;
                let parsed = Second82Protocol::from_bytes(&status_data)?;
                self.status_tx.send_if_modified(|current| {
                    if current.is_none() {
                        *current = Some(parsed.clone());
                        true
                    } else {
                        false
                    }
                });
                parsed
            }
        };
        if zone == 1 {
            Ok(protocol.zone1_state)
        } else {
            Ok(protocol.zone2_state)
        }
    }

    pub async fn handle_mqtt_message(&self, payload: &[u8]) -> Result<()> {
        let msg: MqttCommand = serde_json::from_slice(payload)?;
        info!("Received MQTT command: {:?}", msg);

        // Serialize BLE work: concurrent commands interleave GATT ops on the
        // same device and time out. Holding this across the whole command
        // makes queued commands run one at a time (last one wins for toggles).
        let _command_guard = self.command_lock.lock().await;

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
                        let duration = msg
                            .duration_seconds
                            .unwrap_or(self.config.default_run_seconds);
                        match self.run_command("start", zone_id, duration).await {
                            Ok(s) => s,
                            Err(e) => {
                                error!("Failed to start zone {}: {:#}", zone_id, e);
                                false
                            }
                        }
                    } else {
                        match self.run_command("stop", zone_id, 0).await {
                            Ok(s) => s,
                            Err(e) => {
                                error!("Failed to stop zone {}: {:#}", zone_id, e);
                                false
                            }
                        }
                    };
                    response.success = Some(success);
                }
            }
            "status" => {
                let status = self.get_status(zone_id).await?;
                response.status = Some(if status { 1 } else { 0 });
                let suspend = self
                    .status_cache
                    .borrow()
                    .as_ref()
                    .map(|p| p.suspend_watering)
                    .unwrap_or(false);
                if let Some(obj) = response.extra.as_object_mut() {
                    obj.insert("suspend_watering".to_string(), serde_json::json!(suspend));
                }
                let zone_names = self.zone_names.borrow().clone();
                if let Some(obj) = response.extra.as_object_mut() {
                    obj.remove("zone1_name");
                    obj.remove("zone2_name");
                    if let Some(names) = zone_names {
                        if let Some(name) = names.zone1 {
                            obj.insert("zone1_name".to_string(), serde_json::json!(name));
                        }
                        if let Some(name) = names.zone2 {
                            obj.insert("zone2_name".to_string(), serde_json::json!(name));
                        }
                    }
                }
            }
            _ => {
                error!("Unknown command: {}", msg.cmd);
                return Ok(());
            }
        }

        let resp_payload = serde_json::to_string(&response)?;
        info!(
            "Publishing response to {}: {}",
            self.config.mqtt_pub_topic, resp_payload
        );
        self.mqtt_client
            .publish(&self.config.mqtt_pub_topic, &resp_payload)
            .await?;

        Ok(())
    }

    pub async fn poll_battery(&self) -> Result<()> {
        info!("Polling battery level for {}...", self.config.device_name);
        self.ensure_connection_ready().await?;
        let level = self.ble_client.read_battery().await?;
        info!("Battery level: {}%", level);
        self.db_writer
            .write_battery(&self.config.device_name, level)
            .await?;
        info!("Battery level successfully written to database.");
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BatteryPollingIntervals {
    pub success: Duration,
    pub failure: Duration,
}

impl BatteryPollingIntervals {
    pub const PRODUCTION: Self = Self {
        success: Duration::from_secs(3600),
        failure: Duration::from_secs(60),
    };
}

pub(crate) async fn run_battery_polling_loop(
    app: Arc<App>,
    mut shutdown: watch::Receiver<bool>,
    intervals: BatteryPollingIntervals,
) {
    loop {
        if *shutdown.borrow() {
            break;
        }

        match app.poll_battery().await {
            Ok(_) => {
                if wait_for_shutdown_or_timeout(&mut shutdown, intervals.success).await {
                    break;
                }
            }
            Err(e) => {
                error!(
                    "Battery poll failed: {}. Retrying in {}s...",
                    e,
                    intervals.failure.as_secs()
                );
                if wait_for_shutdown_or_timeout(&mut shutdown, intervals.failure).await {
                    break;
                }
            }
        }
    }
}

async fn wait_for_shutdown_or_timeout(
    shutdown: &mut watch::Receiver<bool>,
    duration: Duration,
) -> bool {
    if *shutdown.borrow() {
        return true;
    }

    tokio::select! {
        changed = shutdown.changed() => changed.is_err() || *shutdown.borrow(),
        _ = tokio::time::sleep(duration) => *shutdown.borrow(),
    }
}

pub(crate) struct BatteryPollingGuard {
    stop_tx: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl BatteryPollingGuard {
    pub(crate) fn start(app: Arc<App>, intervals: BatteryPollingIntervals) -> Self {
        let (stop_tx, stop_rx) = watch::channel(false);
        let task = tokio::spawn(run_battery_polling_loop(app, stop_rx, intervals));
        Self { stop_tx, task }
    }
}

impl Drop for BatteryPollingGuard {
    fn drop(&mut self) {
        let _ = self.stop_tx.send(true);
        self.task.abort();
    }
}

pub(crate) async fn run_valve_event_observer(
    mut status_cache: StatusCache,
    db_writer: Arc<dyn DatabaseWriter>,
    mqtt_client: Arc<dyn MqttClient>,
    device_name: String,
    pub_topic: String,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut prev: Option<Second82Protocol> = None;
    loop {
        if *shutdown.borrow() {
            break;
        }
        tokio::select! {
            changed = status_cache.changed() => {
                if changed.is_err() {
                    break;
                }
                let new = status_cache.borrow().clone();
                if let Some(new) = new {
                    if let Some(prev) = &prev {
                        if prev.zone1_state != new.zone1_state {
                            if let Err(e) = db_writer
                                .write_valve_event(&device_name, "zone1", new.zone1_state)
                                .await
                            {
                                warn!("Failed to write valve event for zone1: {}", e);
                            }
                            push_zone_status(&mqtt_client, &pub_topic, "garden", new.zone1_state).await;
                        }
                        if prev.zone2_state != new.zone2_state {
                            if let Err(e) = db_writer
                                .write_valve_event(&device_name, "zone2", new.zone2_state)
                                .await
                            {
                                warn!("Failed to write valve event for zone2: {}", e);
                            }
                            push_zone_status(&mqtt_client, &pub_topic, "grass", new.zone2_state).await;
                        }
                    }
                    prev = Some(new);
                } else {
                    prev = None;
                }
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
        }
    }
}

pub(crate) struct ValveEventObserverGuard {
    stop_tx: watch::Sender<bool>,
    task: JoinHandle<()>,
}

/// Publish a `status` response for a zone on the c2s topic, reflecting a
/// notification-driven state change (device self-stop, manual button, etc.)
/// so MQTT consumers (e.g. HA entities) see it without polling.
/// Zone-name mapping follows the bridge's canonical contract: zone 1 =
/// "garden", zone 2 = "grass".
async fn push_zone_status(
    mqtt_client: &Arc<dyn MqttClient>,
    pub_topic: &str,
    zone_name: &str,
    state: bool,
) {
    let payload = serde_json::json!({
        "cmd": "status",
        "zone": zone_name,
        "status": if state { 1 } else { 0 },
        "ack": true,
    })
    .to_string();
    if let Err(e) = mqtt_client.publish(pub_topic, &payload).await {
        warn!("Failed to push zone status for {}: {}", zone_name, e);
    }
}

impl ValveEventObserverGuard {
    pub(crate) fn start(
        status_cache: StatusCache,
        db_writer: Arc<dyn DatabaseWriter>,
        mqtt_client: Arc<dyn MqttClient>,
        device_name: String,
        pub_topic: String,
    ) -> Self {
        let (stop_tx, stop_rx) = watch::channel(false);
        let task = tokio::spawn(run_valve_event_observer(
            status_cache,
            db_writer,
            mqtt_client,
            device_name,
            pub_topic,
            stop_rx,
        ));
        Self { stop_tx, task }
    }
}

impl Drop for ValveEventObserverGuard {
    fn drop(&mut self) {
        let _ = self.stop_tx.send(true);
        self.task.abort();
    }
}

const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
const CONNECTION_READY_TIMEOUT: Duration = Duration::from_secs(65);

fn next_backoff(current: Duration) -> Duration {
    let doubled = current.saturating_mul(2);
    if doubled > MAX_BACKOFF {
        MAX_BACKOFF
    } else {
        doubled
    }
}

fn build_password(password: Option<&str>) -> [u8; 4] {
    let mut buf = [0u8; 4];
    match password {
        Some(p) => {
            let bytes = p.as_bytes();
            let n = bytes.len().min(4);
            buf[..n].copy_from_slice(&bytes[..n]);
        }
        None => {
            info!("DEVICE_PASSWORD not set; using default password [0x00,0x00,0x00,0x00]");
        }
    }
    buf
}

async fn run_connection_setup(
    ble_client: &Arc<dyn BleClient>,
    config: &Config,
) -> Result<tokio::sync::mpsc::Receiver<Vec<u8>>> {
    info!(
        "Connection attempt: connecting to {}...",
        config.device_address
    );
    ble_client.connect(&config.device_address).await?;
    info!("Connection established with {}.", config.device_address);

    if let Err(e) = crate::hci::request_interval(&config.device_address, config.conn_interval_ms) {
        warn!(
            "Failed to request {}ms connection interval: {}",
            config.conn_interval_ms, e
        );
    }
    // The peripheral re-negotiates shortly after connect (L2CAP Connection
    // Parameter Update Request), so re-apply our interval over the next minute
    // to make it reliably stick. See hci::spawn_interval_reapply.
    crate::hci::spawn_interval_reapply(config.device_address.clone(), config.conn_interval_ms);

    let password = build_password(config.device_password.as_deref());
    info!("Writing password to ff81...");
    ble_client.write_password(&password).await?;
    info!("Password write succeeded.");

    info!("Subscribing to ff82 notifications...");
    let rx = ble_client
        .subscribe_notifications("0000ff82-0000-1000-8000-00805f9b34fb")
        .await?;
    info!("Subscribed to ff82 notifications.");

    Ok(rx)
}

async fn wait_for_disconnect_or_shutdown(
    shutdown: &mut watch::Receiver<bool>,
    ble_client: &Arc<dyn BleClient>,
) -> bool {
    loop {
        if *shutdown.borrow() {
            return true;
        }
        if !ble_client.is_connected().await {
            info!("BLE connection lost; re-running connection setup");
            return false;
        }
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return true;
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
        }
    }
}

async fn handle_connection_failure(
    ladder: &mut ResilienceLadder,
    controller: &dyn ResilienceController,
    store: &ResilienceStateStore,
    err: &anyhow::Error,
    backoff: Duration,
) {
    let now = Utc::now();
    let action = ladder.on_connection_failure(now);
    let count = ladder.consecutive_failures();
    match action {
        LadderAction::PowerCycle => {
            error!(
                "Connection setup failed ({} consecutive failures): power-cycling adapter",
                count
            );
            if let Err(pe) = controller.power_cycle_adapter().await {
                error!("Power-cycle adapter failed: {}", pe);
            }
            if let Err(se) = store.save(ladder.state()) {
                error!("Failed to persist resilience state: {}", se);
            }
        }
        LadderAction::Reboot => {
            error!(
                "Connection setup failed ({} consecutive failures, rung 3): rebooting host",
                count
            );
            if let Err(se) = store.save(ladder.state()) {
                error!("Failed to persist resilience state before reboot: {}", se);
            }
            if let Err(re) = controller.reboot_host().await {
                error!("Reboot failed: {}", re);
            }
        }
        LadderAction::KeepRetrying => {
            error!(
                "Connection setup failed ({} consecutive failures): 24h reboot cap reached; keep retrying",
                count
            );
        }
        LadderAction::None => {
            error!(
                "Connection setup failed: {}. Retrying in {}s...",
                err,
                backoff.as_secs()
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_connection_supervisor(
    ble_client: Arc<dyn BleClient>,
    config: Config,
    mut shutdown: watch::Receiver<bool>,
    ready_tx: watch::Sender<bool>,
    status_tx: StatusCacheSender,
    zone_names_tx: ZoneNamesCacheSender,
    controller: Arc<dyn ResilienceController>,
    store: ResilienceStateStore,
) {
    let state = match store.load() {
        Ok(s) => s,
        Err(e) => {
            warn!("Failed to load resilience state ({}); starting fresh", e);
            ResilienceState::default()
        }
    };
    let mut ladder = ResilienceLadder::from_state(state);
    let mut backoff = INITIAL_BACKOFF;
    loop {
        if *shutdown.borrow() {
            break;
        }
        match run_connection_setup(&ble_client, &config).await {
            Ok(notif_rx) => {
                ladder.on_connection_success();
                backoff = INITIAL_BACKOFF;
                let _ = status_tx.send(None);
                let _ = zone_names_tx.send(None);
                let zone1 = match ble_client.read_zone1_name().await {
                    Ok(name) => {
                        info!("Decoded zone 1 name: {}", name);
                        Some(name)
                    }
                    Err(e) => {
                        warn!("Failed to read zone 1 name: {}", e);
                        None
                    }
                };
                let zone2 = match ble_client.read_zone2_name().await {
                    Ok(name) => {
                        info!("Decoded zone 2 name: {}", name);
                        Some(name)
                    }
                    Err(e) => {
                        warn!("Failed to read zone 2 name: {}", e);
                        None
                    }
                };
                let _ = zone_names_tx.send(Some(ZoneNames { zone1, zone2 }));
                spawn_notification_consumer(notif_rx, status_tx.clone(), shutdown.clone());
                let _ = ready_tx.send(true);
                if wait_for_disconnect_or_shutdown(&mut shutdown, &ble_client).await {
                    break;
                }
                let _ = ready_tx.send(false);
                let _ = zone_names_tx.send(None);
            }
            Err(e) => {
                handle_connection_failure(&mut ladder, controller.as_ref(), &store, &e, backoff)
                    .await;
                let _ = ready_tx.send(false);
                if wait_for_shutdown_or_timeout(&mut shutdown, backoff).await {
                    break;
                }
                backoff = next_backoff(backoff);
            }
        }
    }
}

fn spawn_notification_consumer(
    mut notif_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    status_tx: StatusCacheSender,
    mut shutdown: watch::Receiver<bool>,
) {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                payload = notif_rx.recv() => {
                    match payload {
                        Some(data) => {
                            match Second82Protocol::from_bytes(&data) {
                                Ok(parsed) => {
                                    let _ = status_tx.send(Some(parsed));
                                }
                                Err(e) => {
                                    warn!("Failed to parse ff82 notification: {}", e);
                                }
                            }
                        }
                        None => break,
                    }
                }
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
            }
        }
    });
}

pub(crate) struct ConnectionGuard {
    stop_tx: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl ConnectionGuard {
    pub(crate) fn start(
        ble_client: Arc<dyn BleClient>,
        config: Config,
        controller: Arc<dyn ResilienceController>,
    ) -> (
        Self,
        watch::Receiver<bool>,
        StatusCache,
        StatusCacheSender,
        ZoneNamesCache,
    ) {
        let (stop_tx, stop_rx) = watch::channel(false);
        let (ready_tx, ready_rx) = watch::channel(false);
        let (status_tx, status_rx) = watch::channel(None);
        let (zone_names_tx, zone_names_rx) = watch::channel(None);
        let store = ResilienceStateStore::new(&config.device_name);
        let task = tokio::spawn(run_connection_supervisor(
            ble_client,
            config,
            stop_rx,
            ready_tx,
            status_tx.clone(),
            zone_names_tx,
            controller,
            store,
        ));
        (
            Self { stop_tx, task },
            ready_rx,
            status_rx,
            status_tx,
            zone_names_rx,
        )
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        let _ = self.stop_tx.send(true);
        self.task.abort();
    }
}

pub async fn run_app() -> Result<()> {
    tracing_subscriber::fmt::init();

    crate::hci_monitor::spawn_hci_monitor();

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
        &format!("rshunterbtt-{}", config.device_name),
    );
    let mqtt_client = Arc::new(mqtt_client_impl);

    let db_writer = Arc::new(crate::database::InfluxDbWriter::new(
        &config.influxdb_url,
        &config.influxdb_token,
        &config.influxdb_org,
        &config.influxdb_bucket,
    ));
    let db_writer_observer = db_writer.clone();

    let app = App::new(
        config.clone(),
        ble_client.clone(),
        mqtt_client.clone(),
        db_writer,
    );

    let controller = Arc::new(crate::dbus_control::SystemResilienceController::default());
    let (connection_guard, connection_ready, status_cache, status_tx, zone_names) =
        ConnectionGuard::start(ble_client, config.clone(), controller);
    let status_cache_observer = status_cache.clone();
    let app = Arc::new(
        app.with_connection_ready(connection_ready)
            .with_status_cache(status_cache, status_tx)
            .with_zone_names(zone_names),
    );

    let _battery_polling =
        BatteryPollingGuard::start(app.clone(), BatteryPollingIntervals::PRODUCTION);
    let _valve_event_observer = ValveEventObserverGuard::start(
        status_cache_observer,
        db_writer_observer,
        mqtt_client.clone(),
        config.device_name.clone(),
        config.mqtt_pub_topic.clone(),
    );
    let _connection = connection_guard;

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
                error!(
                    "MQTT loop error: {}. Returning to supervisor for restart...",
                    e
                );
                return Err(anyhow!("MQTT EventLoop error: {}", e));
            }
        }
    }
}
