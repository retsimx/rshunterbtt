# rshunterbtt

A resilient Rust-based MQTT/BLE bridge for Hunter BTT irrigation controllers, migrated from the original Python implementation (`pyHunterBTT`).

## Overview

`rshunterbtt` acts as a bridge between an MQTT broker and Hunter BTT Bluetooth Tap Timers. It allows for remote control (start/stop) and monitoring (status, battery level) of irrigation zones via MQTT messages. Each instance is designed to handle exactly one Hunter BTT device, typically deployed on a Raspberry Pi Zero W, and holds a persistent authenticated BLE connection to it, driven by the device's own status notifications rather than polling.

## Features

- **Persistent BLE connection**: holds a single GATT connection open per device
  — authenticated (4-byte password written to `ff81`) and subscribed to zone
  notifications — instead of reconnecting for every operation.
- **Notification-driven status**: subscribes to the `ff82` status characteristic
  and caches pushed state changes, so a self-timed stop or a physical button
  press is observed immediately rather than polled for.
- **Resilient supervisor**: reconnects with exponential backoff after any drop;
  a `SharedAdapter` is reused across MQTT instance restarts and rebuilt after five
  consecutive acquisition failures, without host or adapter escalation.
- **Interval guard**: requests a configurable BLE connection interval and watches
  the controller's HCI event stream, re-applying the interval whenever the
  peripheral re-negotiates it back to fast.
- **MQTT control + telemetry**: zone commands and status over MQTT; battery
  telemetry written directly to InfluxDB, bypassing MQTT.
- **Dependency injection**: Rust traits with `mockall` for comprehensive unit and
  integration testing.
- **Cross-compilation**: configured for `cross` to target
  `arm-unknown-linux-musleabihf` (Raspberry Pi Zero W / ARMv6).
- **Production ready**: OpenRC service script and detailed `tracing` logs.

## Hardware Mapping

The project supports multiple controllers across different Raspberry Pi devices:

- **Rear Controller**: `<rear-controller-mac>` (Deployed on `<rear-host>`)
- **Front Controller**: `<front-controller-mac>` (Deployed on `<front-host>`)
- **Front Small Controller**: `<small-front-controller-mac>` (Deployed on `<small-front-host>`)

## Documentation

Reference notes on the protocol and design live in [`docs/`](docs/README.md):

- [Advertising vs GATT](docs/protocol/advertising-vs-gatt.md) — why status is GATT-only
- [GATT profile](docs/protocol/gatt-profile.md) — services, characteristics, notifications
- [AIS protocol](docs/protocol/ais-protocol.md) — framing, command IDs, the zone-state enum
- [Architecture](docs/architecture.md) — the persistent-connection design
- [Connection interval](docs/connection-interval.md) — latency vs battery, and the interval guard
- [Battery life](docs/battery-life.md) — real-world figures from InfluxDB
- [HA integration](docs/ha-integration.md) — protocol features for a home-automation consumer
- [Integration strategy](docs/integration-strategy.md) — the decision record
- [Protocol coverage and gaps](docs/gaps.md) — what is and isn't implemented

## Configuration

Configuration is managed via a `.env` file in the working directory:

```env
# BLE Configuration
DEVICE_ADDRESS=<device-mac>
DEVICE_NAME=rear
# DEVICE_PASSWORD=<plain ASCII string, up to 4 characters>

# MQTT Configuration
MQTT_BROKER=<broker-address>
MQTT_PORT=1883
MQTT_SUB_TOPIC=irrigation/s2c/rear/#
MQTT_PUB_TOPIC=irrigation/c2s/rear

# Run-duration failsafe (seconds)
# DEFAULT_RUN_SECONDS=7200

# BLE connection interval (ms; 8-4000, default 4000). Lower = faster MQTT
# command turnaround but more radio connection events (battery drain).
# Empirical: 60ms -> ~0.5s commands, 1000ms -> ~5s, 4000ms -> ~21s. The
# Hunter BTT peripheral re-negotiates back to ~48-60ms; the bridge's interval
# guard (HCI monitor) detects and re-applies this value.
# CONN_INTERVAL_MS=1000

# InfluxDB Configuration
INFLUXDB_URL=<broker-url>
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
Topic: `irrigation/s2c/<device_name>/<zone>`
Payload: `{"cmd": "status", "zone": "grass"}`

The bridge subscribes to `irrigation/s2c/<device_name>/#`, so the status
command arrives on the same topic family as `on_off`.

### Responses (C2S)
Topic: `irrigation/c2s/<device_name>`

The bridge publishes a response for every command:

- `on_off`: `{"cmd":"on_off","zone":"grass","on_off":true,"success":true,"ack":true}`
- `status`: `{"cmd":"status","zone":"grass","status":1,"ack":true,"suspend_watering":false}`

`ack` reports whether the command reached the device; `success` reports whether
the BLE write actually took effect.

The bridge also **pushes** a `status` response whenever the device reports a
zone-state change over `ff82` — a self-timed stop, a physical button press — so
consumers do not have to poll for it:
`{"cmd":"status","zone":"garden","status":0,"ack":true}`.

### Zone-state values

The `status` field is an **enum, not a boolean**. The device reports `1` when a
zone is idle, `0` for standby, `2` for a scheduled-watering reminder, and `5`,
`9`, or `17` while it is actually watering. Consumers should treat
`{5, 9, 17}` as "on".

### Zone-name mapping

The device maps the zone name `grass` to zone 2 and anything else to zone 1, so
the name must be exactly `grass` or `garden` — a typo silently targets zone 1.

## Connection Recovery

Normal connection failures use only exponential backoff (1s → 60s cap). The
process creates one `SharedAdapter` in `run_app` and reuses it across MQTT
instance restarts. After five consecutive failed acquisitions, the supervisor
runs bounded Peripheral cleanup, clears the cached Adapter, and calls
`setup_central` / `Manager::new()` once more to reinitialize the normal BLE
session. A successful setup resets the failure count. MQTT instance cancellation
and guard shutdown also run the same bounded, exactly-once cleanup.

There is no legacy escalation ladder: normal failures do not power-cycle the
BLE adapter, write the BlueZ `Powered` property, reboot the host, or persist
ladder state.

### HCI monitoring

`src/hci_monitor.rs` is spawned once for process lifetime. It re-applies the
configured connection interval whenever our own link reports an off-target
`LE Connection Update Complete` (rate-limited to one attempt per 3 s), and logs
HCI disconnection and controller status events, including `0x2005`. It never
uses HCI status to gate or restart connection recovery.

### Required host permissions

- **`CAP_NET_ADMIN`** (i.e. run as root) for the raw HCI sockets used to request
  the connection interval and to run the interval guard — the monitor decodes
  `LE Connection Update Complete` events and re-applies the configured interval
  when the peripheral re-negotiates it away.

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

### Connection interval

The bridge requests its configured connection interval (`CONN_INTERVAL_MS`)
at runtime via a pure-Rust HCI connection-parameter update (an `LE Connection
Update` command sent over a raw HCI socket) — no external tools required. This
is necessary because BlueZ's central-role connection otherwise uses the
peripheral's advertised preferred parameters for the link.

The request is scoped to the bridge's own connection (the HCI handle is
resolved from `DEVICE_ADDRESS`), so sharing an adapter with other BLE devices
is safe.

The Hunter BTT peripheral re-negotiates the interval back towards ~50ms shortly
after connecting (and intermittently afterwards), which BlueZ honours. The
interval guard watches the controller's event stream and re-applies the
configured value whenever the peripheral pulls the interval away from target.

1. Copy the cross-compiled binary to `/root/rshunterbtt/rshunterbtt`.
2. Create `/root/rshunterbtt/.env` with the correct device settings.
3. Install the OpenRC script to `/etc/init.d/rshunterbtt`.
4. Enable and start:
   ```bash
   rc-update add rshunterbtt default
   rc-service rshunterbtt start
   ```

## License

MIT — see [LICENSE](LICENSE).
