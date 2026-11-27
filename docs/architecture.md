# Architecture

The design `rshunterbtt` is built around, and the live-probe findings that
motivated it. Status: **implemented** — see the repository for the current code.

## Context

The original bridge connected briefly — an hourly battery poll plus on-demand
reads — and the connection dropped between operations, so it was mostly
disconnected in practice. Live probing established that a persistent connection
with `ff82` notifications is viable:

| Test | Result |
|---|---|
| Idle connection survival | PASS — the device holds an idle connection |
| Notification while idle | PASS — the device pushes `ff82` on a button press |
| Connection parameters | PASS — the device accepts up to 4000ms (BLE max) |
| Reconnection | PASS — reconnect + re-subscribe works |

## Connection model

- **One persistent connection per device**, held open indefinitely rather than
  re-established for every operation.
- **Authentication** — the 4-byte password is written to `ff81` on every
  connection setup.
- **Subscription** — the bridge subscribes to `ff82` for zone-state
  notifications; the device pushes on state change, so no polling is needed.
- **Connection interval** — configurable via `CONN_INTERVAL_MS`. The 4000ms
  maximum was the original choice for battery (a 66× reduction in connection
  events over the 60ms default), but it costs ~21s per command; the deployment
  runs at 1000ms. See
  [connection interval](connection-interval.md).
- **Battery** — `2a19` is read on an hourly schedule and written directly to
  InfluxDB.
- **Setup is atomic** — the password write and the notification subscription do
  not survive a disconnect, so every reconnect re-runs the full sequence.

### Zone-state decoding

`ff82` is a 14-byte payload: `0` enabled, `1` suspend, `2` z1Enabled, `3`
z1Mode, `4` z1EnableManual, `5` z1ExtManual, `6` z2Enabled, `7` z2Mode, `8`
z2EnableManual, `9` z2ExtManual, **`10` z1State, `11` z2State**, `12`
z1Conflict, `13` z2Conflict.

The zone state at bytes 10/11 is an **enum, not a boolean** — a zone waters iff
the state is `5`, `9` or `17`; `0` (standby), `1` (idle) and `2` (scheduled
reminder) are off. See [AIS protocol](protocol/ais-protocol.md).

### Notification handling

On each `ff82` notification the bridge parses the payload, updates its state
cache, writes a state-transition event to InfluxDB, and publishes a status
response to MQTT — so consumers learn about changes, including runs the bridge
did not start, without polling.

### Reconnection

A dropped connection is re-established with exponential backoff, and the
service re-runs full connection setup including re-subscription (`ff82` does
not survive a drop).

### Shared Adapter and controller diagnostics

`run_app` creates one `SharedAdapter` and reuses it across MQTT-triggered
`run_app_instance` restarts. The shared slot retains the btleplug Adapter and
scan-active state. After five consecutive failed acquisitions, the supervisor
clears and rebuilds the Adapter via `setup_central` / `Manager::new()`; a
successful setup resets the failure count.

Normal Device-not-found, start-scan, and connection failures use backoff and
retry/reinitialize only. There is no host reboot, adapter power-cycle, or
persistent escalation state. `hci_monitor` runs once for process lifetime and
logs disconnection/controller status for diagnostics; it never gates or
restarts recovery based on HCI status.

## Battery expectation

- The BLE radio is likely a minority of the device's total draw (the solenoid
  and MCU dominate), so connection-strategy changes are not expected to move
  the battery number much.
- The measured ~10 months is the connect-briefly baseline; the cost of a
  persistent connection at any interval remains unmeasured. Observe it in
  production via the InfluxDB battery telemetry. See [battery life](battery-life.md).

## Open items

- **Peripheral latency** — whether the device accepts non-zero latency for
  lower power.
- **Password (`ff81`)** — whether the device enforces it; the bridge sends the
  configured value (or the documented default) regardless.

## HA integration

See [HA integration](ha-integration.md) for the protocol features available to a
home-automation consumer.
