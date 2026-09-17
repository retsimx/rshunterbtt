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
- **Resilient supervisor**: reconnects with exponential backoff after any drop,
  and escalates through a recovery ladder (BLE adapter power-cycle, then host
  reboot) when connection-setup failures persist.
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

- **Rear Controller**: `18:04:ED:53:6D:20` (Deployed on `rearpiw`)
- **Front Controller**: `18:04:ED:56:9F:71` (Deployed on `bedroompiw`)
- **Front Small Controller**: `F4:60:77:2F:3F:78` (Deployed on `frontpiw`)

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
- **`CAP_NET_ADMIN`** (i.e. run as root) for the raw HCI sockets used to request
  the connection interval and to run the interval guard — the monitor decodes
  `LE Connection Update Complete` events and re-applies the configured interval
  when the peripheral re-negotiates it away.

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

The bridge requests its configured connection interval (`CONN_INTERVAL_MS`)
at runtime via a pure-Rust HCI connection-parameter update (an `LE Connection
Update` command sent over a raw HCI socket) — no external tools required. This
is necessary because BlueZ's central-role connection otherwise uses the
peripheral's advertised preferred parameters, not the `main.conf` defaults.

The Hunter BTT peripheral re-negotiates the interval back towards ~50ms shortly
after connecting (and intermittently afterwards), which BlueZ honours. The
bridge handles this two ways: connection setup deliberately connects,
disconnects, and reconnects — the peripheral only re-negotiates on its first
connection after boot — and the interval guard re-applies the configured value
whenever it observes the interval drift away from target.

Keep BlueZ's default connection parameters in `/etc/bluetooth/main.conf` in
sync with `CONN_INTERVAL_MS` as a fallback for when the runtime request is
rejected. The interval fields are in ×1.25ms steps, i.e. `interval_ms × 0.8`
(so 1000ms → 800). Add to the `[LE]` section:

```ini
[LE]
MinConnectionInterval=800
MaxConnectionInterval=800
ConnectionLatency=0
ConnectionSupervisionTimeout=2000
```

The supervision timeout is in ×10ms steps (`2000 × 10ms = 20000ms`, above the
mandatory floor of `2×(1+latency)×interval` and within the 32000ms ceiling).

Apply the change with `rc-service bluetooth restart` (Alpine/OpenRC hosts — no
systemd), then reboot the host so the interval is applied cleanly.

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
