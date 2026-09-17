# GATT Profile

The device profile (which services/characteristics exist and which are
notification-enabled) is defined in the app's per-device profile definitions.
Two generations are supported.

## Device classification

During scanning the app matches the **advertised service UUID** to classify a
device:

- **First generation** (device name starts with `BTT`): service
  `0000fcc0-0000-1000-8000-00805f9b34fb`
- **Second generation**: service `0000ff80-0000-1000-8000-00805f9b34fb`

## Standard GATT services (both generations)

| Service | UUID | Characteristic | UUID | Access |
|---|---|---|---|---|
| Battery | `0000180f` | BatteryLevel | `00002a19` | read |
| Generic Access | `00001800` | DeviceName | `00002a00` | read (2nd gen) |
| Device Information | `0000180a` | FirmwareVersion | `00002a26` | read (2nd gen) |
| Device Information | `0000180a` | ManufacturerName | `00002a29` | read (2nd gen) |

## Second generation — service `0000ff80` (ble-80)

Characteristic UUIDs are `0000ffXX-0000-1000-8000-00805f9b34fb`. The low byte
(`XX`) is the protocol/command ID. `(N)` marks notification-enabled.

| Char | Protocol | Meaning |
|---|---|---|
| `ff81` | Second_81 | password (auth) |
| `ff82` (N) | Second_82 | **zone status** (byte 0 enabled, 1 suspend, 2 z1Enabled, 3 z1Mode, 4 z1EnableManual, 5 z1ExtManual, 6 z2Enabled, 7 z2Mode, 8 z2EnableManual, 9 z2ExtManual, 10 z1State, 11 z2State, 12 z1Conflict, 13 z2Conflict). Bytes 10/11 are a **state enum, not a boolean** — a zone waters iff the state is `5`, `9` or `17` (`1` = idle, `0` = standby, `2` = scheduled reminder). See [AIS protocol](ais-protocol.md). |
| `ff83` | Second_83 | overall status (enabled, suspend, runAll HH:MM:SS, zone config) |
| `ff84` | Second_84 | clock |
| `ff85`–`ff88` | Second_85–88 | program schedules (TM/CYC, start/end) |
| `ff8a` (N) | Second_8a | active run times (tmRun) |
| `ff8b`–`ff8e` | Second_8b–8e | zone config / schedules |
| `ff8f` (N) | Second_8f | active run times (cmRun/mRun/emRun) |
| `ff90`/`ff91` | Second_90/91 | zone names |
| `ff92`–`ff9b` | Second_92–9B | zone image transfer (`ff99`/`ff9b` are N) |
| `ff9c` | Second_9C | flow config |
| `ff9d` | Second_9D | watering window start/end |
| `ff9e` (N) | Second_9E | irrigation records (zone, timestamp, run/infiltrate sec) |
| `ff9f` | Second_9F | debug |
| `ffa0`/`ffa1` | Second_A0/A1 | start times zones 5–8 |
| `ffa2` (N) | Second_A2 | total irrigation record count |
| `ffa3` (N) | Second_A3 | irrigation records |

## First generation — service `0000fcc0` (ble-c0)

Characteristic UUIDs are `0000fcXX-…`. `(N)` marks notification-enabled.

| Char | Protocol | Meaning |
|---|---|---|
| `fcc1` | First_C1 | password (header `0x51`) |
| `fcc2` | First_C2 | current mode (status) |
| `fcc3` | First_C3 | select mode (command) |
| `fcc4` | First_C4 | clock |
| `fcd1` (N) | First_D1 | **detail state / zone state** (header `0x61`) |
| `fcd2` (N) | First_D2 | duration |
| `fcd3` | First_D3 | schedule |
| `fcd4` | First_D4 | start times |
| `fcd5` | First_D5 | delay |
| `fcd6` (N) | First_D6 | |
| `fcd7` | First_D7 | auto |
| `fcd8` | First_D8 | off |
| `fcd9` (N) | First_D9 | control + minute (manual run) |
| `fce1` (N) | First_E1 | **detail state / zone state** (header `0x71`) |
| `fce2`–`fce8` | First_E2–E8 | start times, week |
| `fce9`/`fcea` | First_E9/EA | auto / off |
| `fceb` (N) | First_EB | control + minute (manual run) |
| `fcf1`/`fcf2` | First_F1/F2 | empty payload (start/stop commands) |

## Notification setup

During service discovery, every characteristic flagged as notification-enabled
is subscribed via the platform's notification API plus a CCCD write to
`00002902`.

Notification-enabled characteristics:

- **Second gen**: `ff82, ff8a, ff8f, ff99, ff9b, ffa2, ff9e, ffa3`
- **First gen**: `fd1, fd2, fd6, fd9, fe1, fe6, fe7, feb`

Incoming notifications are dispatched by characteristic UUID, parsed into the
matching protocol object, and forwarded to the caller.
