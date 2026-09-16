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

## MQTT Interface

### Commands (S2C)
Topic: `irrigation/s2c/<device_name>/<zone>`
Payload: `{"cmd": "on_off", "zone": "grass", "on_off": true}`

### Status (S2C)
Topic: `irrigation/s2c/<device_name>/status`
Payload: `{"cmd": "status", "zone": "grass"}`

### Responses (C2S)
Topic: `irrigation/c2s/<device_name>`
Payload example: `{"cmd":"on_off","zone":"grass","on_off":true,"success":true,"ack":true}`

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
