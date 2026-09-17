# Do Hunter BTT devices broadcast status via BLE advertisements/beacons?

**Short answer: No.** Status is only obtainable through an **active BLE GATT
connection** (characteristic reads, writes, and notifications). The devices do
not include status data in their advertising packets.

## What the advertisement is used for

BLE scanning in the app is used **only for device discovery**. From each scan
result the app extracts exactly three things:

1. **Device name** — for display.
2. **RSSI** — for signal-strength display.
3. **Advertised service UUIDs** — to classify the device as First or Second
   generation.

Evidence — from the app's scan handling:

- Every scan callback, the modern and the legacy path alike, forwards only
  three things: the device **name**, the **RSSI**, and the advertised
  **service-UUID list**.
- The scan-record parser **does** decode the manufacturer-specific data and the
  service data from the advertisement, but the scan path never reads those
  fields. No status is ever decoded from the advertising packet.
- The advertised service UUID is matched against `0000fcc0` (1st gen) /
  `0000ff80` (2nd gen) purely to classify the device and record its
  name/address/RSSI.

## How status is actually obtained

All status requires an active GATT connection (connect → discover services →
per-characteristic transactions):

| Status | Mechanism | Characteristic |
|---|---|---|
| Battery level | GATT **read** | `00002a19` (Battery Service `0000180f`) |
| Zone on/off state (1st gen) | GATT **notification** | `0000fd1`, `0000fe1` (service `0000fcc0`) |
| Zone on/off state (2nd gen) | GATT **notification** | `0000ff82` (service `0000ff80`) |
| Run times (2nd gen) | GATT **notification** | `0000ff8a`, `0000ff8f` |
| Irrigation records (2nd gen) | GATT **notification** | `0000ff9e`, `0000ffa2`, `0000ffa3` |

Notifications arrive per characteristic UUID and are dispatched as the parsed
protocol value.

## Implication for rsHunterBTT

- There is **no beacon/passive mode** to exploit: to read zone state or
  battery you must connect and read/notify over GATT. This matches what
  `rsHunterBTT` already does (GATT reads of `0000ff82` and `00002a19`).
- The flakiness in `rsHunterBTT` is therefore **not** due to a missing
  advertisement-based status path; it is due to the GATT connection /
  read / write reliability itself (connection maintenance, retry, and
  notification subscription), which is where improvement effort should focus.
- **Note on connection persistence:** although `rsHunterBTT`'s code is
  designed for a persistent connection (no `disconnect()`), the runtime logs
  show a full reconnect before every operation (`le-connection-abort-by-local`).
  In practice the device is **mostly disconnected**, connecting briefly each
  hour and on commands. See [integration strategy](../integration-strategy.md) for the full analysis.
