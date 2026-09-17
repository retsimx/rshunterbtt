# Protocol coverage and known gaps

What `rshunterbtt` implements of the Hunter BTT protocol, and what it
deliberately does not. Items under **not implemented** are candidates for
future work rather than defects.

## Implemented

- **Persistent connection** — one authenticated GATT connection per device, held
  open and rebuilt in full after any drop. See [architecture](architecture.md).
- **Device authentication** — the 4-byte password written to `ff81` on every
  connection setup.
- **Notification-driven status** — a subscription to `ff82`; zone state is
  decoded as the enum documented in [AIS protocol](protocol/ais-protocol.md)
  and pushed to MQTT on every change, including runs the bridge did not start.
- **Commands** — start/stop with an optional run duration, via the device's
  `ff83` / `ff86` / `ff8b` writes.
- **Battery telemetry** — `2a19` read on an hourly poll and written directly to
  InfluxDB.
- **Valve state events** — every observed zone transition is written to
  InfluxDB as an `irrigation` measurement.
- **Connection interval control** — a configurable interval requested over raw
  HCI, with an event-driven guard that re-applies it whenever the peripheral
  re-negotiates. See [connection interval](connection-interval.md).
- **Host recovery** — an escalation ladder that power-cycles the BLE adapter
  and, as a last resort, reboots the host.

## Not implemented

- **Zone-conflict bytes** — `ff82` bytes 12/13. The OEM app parses them but
  never reads them anywhere, so their meaning is unverified and they are not
  surfaced.
- **First-generation devices** — the bridge targets the second-generation
  profile (`0000ff80`) exclusively, matching the deployed hardware. First-gen
  devices (`0000fcc0`, in-payload framing) are unsupported.
- **Program schedules and clock** — `ff84`–`ff88`. All watering is driven
  externally, so the on-device schedule is unused.
- **Active run times and irrigation history** — `ff8a`, `ff8f`, `ff9e`,
  `ffa2`, `ffa3`. Available via notifications; not consumed.
- **Flow sensor configuration** — `ff9c`.
- **Zone names** — `ff90` / `ff91`. The bridge attempts a read and falls back
  gracefully when the characteristic is absent — as it is on the deployed
  hardware.
- **MQTT authentication / TLS** — the bridge connects to a plain MQTT broker.
  Authentication and transport security are deployment concerns, not
  implemented here.

## Confirmed against the OEM app

- The characteristic UUID scheme
  `{protocol_id:08x}-0000-1000-8000-00805f9b34fb` matches the OEM
  protocol enum exactly (see `src/protocol.rs`).
- Battery uses the standard GATT `2a19` characteristic.
- The `ff82` zone-state byte is an enum, not a boolean — see
  [AIS protocol](protocol/ais-protocol.md). An earlier revision of the bridge
  read the manual-enable flags (bytes 4/8) as zone state; it now reads the true
  state (bytes 10/11).
