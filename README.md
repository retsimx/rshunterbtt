# rshunterbtt

A resilient Rust-based MQTT/BLE bridge for Hunter BTT irrigation controllers, migrated from the original Python implementation (`pyHunterBTT`).

## Overview

`rshunterbtt` acts as a bridge between an MQTT broker and Hunter BTT Bluetooth Tap Timers. It allows for remote control (start/stop) and monitoring (status, battery level) of irrigation zones via MQTT messages. Each instance is designed to handle exactly one Hunter BTT device, typically deployed on a Raspberry Pi Zero W.

## Features

- **Asynchronous Architecture**: Built on `tokio` for efficient handling of concurrent MQTT and BLE operations.
- **Resilient Supervisor**: High-level supervisor loop ensures the service automatically reconnects to MQTT and retries BLE operations after failures or reboots.
- **Dependency Injection**: Utilizes Rust traits and `mockall` for comprehensive unit and integration testing.
- **Cross-Compilation**: Fully configured for `cross` to target `arm-unknown-linux-musleabihf` (Raspberry Pi Zero W / ARMv6).
- **Production Ready**: Includes OpenRC service scripts and detailed `tracing` logs with raw GATT byte debugging.

## Hardware Mapping

The project supports multiple controllers across different Raspberry Pi devices:

- **Rear Controller**: `18:04:ED:53:6D:20` (Deployed on `rearpiw`)
- **Front Controller**: `18:04:ED:56:9F:71` (Deployed on `bedroompiw`)
- **Front Small Controller**: `F4:60:77:2F:3F:78` (Deployed on `frontpiw`)

## Configuration

Configuration is managed via a `.env` file in the working directory:

```env
# BLE Configuration
DEVICE_ADDRESS=18:04:ED:53:6D:20
DEVICE_NAME=rear
# DEVICE_PASSWORD=<plain ASCII string, up to 4 characters>

# MQTT Configuration
MQTT_BROKER=10.0.21.245
MQTT_PORT=1883
MQTT_SUB_TOPIC=irrigation/s2c/rear/#
MQTT_PUB_TOPIC=irrigation/c2s/rear

# Run-duration failsafe (seconds)
# DEFAULT_RUN_SECONDS=7200

# BLE connection interval (ms; 8-4000, default 4000). Lower = faster MQTT
# command turnaround but more radio connection events (battery drain).
# Empirical: 60ms -> ~0.5s commands, 1000ms -> ~5s, 4000ms -> ~21s. The
# Hunter BTT peripheral re-negotiates back to ~48-60ms; the bridge's interval
# guard (HCI monitor) detects and re-applies this value. Keep
# /etc/bluetooth/main.conf [LE] Min/MaxConnectionInterval in sync
# (interval_ms * 0.8).
# CONN_INTERVAL_MS=1000

# InfluxDB Configuration
INFLUXDB_URL=http://10.0.25.10:8086
INFLUXDB_TOKEN=your_token_here
INFLUXDB_ORG=home
INFLUXDB_BUCKET=sprinkler
```

### Device Password (`DEVICE_PASSWORD`)

The device password is a plain ASCII string of up to 4 characters. On
connect, the bridge writes it to characteristic `ff81` as raw bytes:
the password's bytes are copied into a fixed 4-byte buffer at index 0
for `min(4, len)` bytes, zero-padded right (or truncated) to 4 bytes.
No hex-encoding, null-termination, or length prefix is used (OEM
parity).

If `DEVICE_PASSWORD` is unset, the bridge uses the default
`[0x00, 0x00, 0x00, 0x00]`. This is an **assumption, not a verified
OEM default** — a repo-wide search of decompiled OEM sources found no
hardcoded factory-default password. If your device requires a password,
set `DEVICE_PASSWORD` explicitly.

### Run-duration failsafe (`DEFAULT_RUN_SECONDS`)

`DEFAULT_RUN_SECONDS` sets the default run duration (in seconds) used
when an `on_off` start command does not specify a `duration_seconds`.
The default is `7200` (2 hours). It acts as a failsafe so a start
command without an explicit duration still stops the zone after a
bounded time.

## MQTT Interface

### Commands (S2C)
Topic: `irrigation/s2c/<device_name>/<zone>`
Payload: `{"cmd": "on_off", "zone": "grass", "on_off": true}`

The `on_off` command accepts an optional `duration_seconds` field:

- `duration_seconds` specifies the run duration (in seconds) when
  `on_off` is `true`. If omitted, the run duration falls back to
  `DEFAULT_RUN_SECONDS` (default `7200`).
- It is ignored when `on_off` is `false` — stop remains immediate.
- A value that cannot be represented in the device's byte-field
  encoding (i.e. it overflows the field) returns `success: false`.

Example with an explicit duration:
`{"cmd": "on_off", "zone": "grass", "on_off": true, "duration_seconds": 1800}`

### Status (S2C)
Topic: `irrigation/s2c/<device_name>/status`
Payload: `{"cmd": "status", "zone": "grass"}`

### Responses (C2S)
Topic: `irrigation/c2s/<device_name>`
Payload example: `{"cmd":"on_off","zone":"grass","on_off":true,"success":true,"ack":true}`

## Connection Resilience Escalation Ladder

The connection supervisor escalates through a ladder of recovery actions as
connection-setup failures accumulate. The ladder is pure decision logic
(`src/resilience.rs`); the actual recovery actions are performed by a
`ResilienceController` (`src/dbus_control.rs`, `src/reboot.rs`).

### Thresholds and rungs

| Rung | Trigger | Action |
|------|---------|--------|
| 1 | First failures | Keep retrying with exponential backoff (1s → 60s cap). |
| 2 | **5 consecutive failures within a rolling 10-minute window** | **Power-cycle the BLE adapter** via D-Bus (`Powered=false`, 1s delay, `Powered=true`). Rate-limited to once per 15 minutes. |
| 3 | **15 consecutive failures** (5 + 10) after a power-cycle has been attempted | **Reboot the host** via `reboot(2)` (`RB_AUTOBOOT`). Rate-limited to once per 30 minutes, max 3 reboots per 24 hours. |
| 4 | 24h reboot cap (3) reached | Stop escalating; keep retrying with backoff. |

A successful connection resets the consecutive-failure counter and the failure
window. Every rung transition and rate-limit skip is logged at `error`/`warn`
with the current failure count.

### State file

Escalation state is persisted durably (write + flush + `fsync`, including the
directory) so it survives a reboot and is loaded on startup before any
escalation decision:

```
/var/lib/rshunterbtt/<device_name>-resilience-state.json
```

Format (pretty-printed JSON):

```json
{
  "last_power_cycle_at": "2026-01-01T00:00:00Z",
  "reboot_timestamps": ["2026-01-01T00:00:00Z"]
}
```

- `last_power_cycle_at`: timestamp of the most recent adapter power-cycle.
- `reboot_timestamps`: list of host reboot timestamps; entries older than 24h
  are pruned on load and on each failure evaluation.

### Required host permissions

- **Root** for D-Bus adapter control (`Powered` property on `org.bluez`) and
  for `reboot(2)`.
- **`CAP_NET_ADMIN`** for the raw HCI monitor socket (diagnostic only; the
  monitor is non-gating and logs HCI disconnection/address events).

### Resetting the rate-limit state manually

To reset the escalation state (e.g. for testing), delete the state file and
restart the service:

```bash
rm -f /var/lib/rshunterbtt/<device_name>-resilience-state.json
rc-service rshunterbtt restart
```

The supervisor will start from a clean state (no pending power-cycle or reboot
rate limits).

## Development

### Running Tests
```bash
cargo test --features mockall
```

### Cross-Compilation (ARMv6)
```bash
cross build --target arm-unknown-linux-musleabihf --release
```

## Deployment

### Required host BLE configuration (one-time)

The bridge requests a **4000ms** connection interval at runtime via a
pure-Rust HCI connection-parameter update (an `LE Connection Update`
command sent over a raw HCI socket) — no external tools required. This is
necessary because BlueZ's central-role connection otherwise uses the
peripheral's advertised preferred parameters (100–200ms), not the
`main.conf` defaults.

In addition, configuring the BlueZ default connection parameters in
`/etc/bluetooth/main.conf` is **recommended** as a fallback default. Add
the following to the `[LE]` section:

```ini
[LE]
MinConnectionInterval=3200
MaxConnectionInterval=3200
ConnectionLatency=0
ConnectionSupervisionTimeout=2000
```

Units: the interval fields are in ×1.25ms steps (`3200 × 1.25ms =
4000ms`); the supervision timeout is in ×10ms steps (`2000 × 10ms =
20000ms`, above the mandatory floor of `2×(1+latency)×interval =
8000ms` and within the 32000ms ceiling).

Apply the change with `rc-service bluetooth restart` (Alpine/OpenRC
hosts — no systemd) or a host reboot.

1. Copy the cross-compiled binary to `/root/rshunterbtt/rshunterbtt`.
2. Create `/root/rshunterbtt/.env` with the correct device settings.
3. Install the OpenRC script to `/etc/init.d/rshunterbtt`.
4. Enable and start:
   ```bash
   rc-update add rshunterbtt default
   rc-service rshunterbtt start
   ```
