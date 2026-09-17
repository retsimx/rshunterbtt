# AIS Application Protocol

The AIS layer is the application protocol layered on top of GATT. Each
protocol maps 1:1 to a GATT characteristic, and its numeric protocol ID becomes
the low bytes of the characteristic UUID:

```
Second_82(65410) -> 0000ff82-0000-1000-8000-00805f9b34fb
First_c1(64705)  -> 0000fcc1-0000-1000-8000-00805f9b34fb
```

## Message framing

### Second generation

**No header, no length, no checksum.** The payload is the raw data bytes; the
characteristic UUID itself identifies the command.

- `Second_81` payload = 4-byte password
- `Second_82` payload = 14 status bytes

### `Second_82` zone-state bytes (10/11) — enum, not boolean

The zone-state byte is a **multi-value enum**, not a 0/1 on/off flag.
Ground truth: the OEM app's status handling and live button-press probes,
where a physical button press produced byte 10 = `0x09` (started) → `0x01`
(stopped).

| Value | Meaning | Watering? |
|---|---|---|
| 0 | standby / off | no |
| 1 | idle / monitoring (normal) | no |
| 2 | scheduled-watering reminder | no |
| 5 | watering (command-started) | **yes** |
| 9 | watering (ext-manual started) | **yes** |
| 17 (`0x11`) | watering (manual button) | **yes** |

A zone is **ON (watering) iff the state byte is 5, 9 or 17**. States 0, 1
and 2 must NOT be treated as on — a device at rest reports `1` (idle).

### First generation

Adds an in-payload frame:

```
[ header (1 byte = command ID) ] [ length (1 byte = payload length) ] [ payload ]
```

The header byte is the command ID (e.g. `First_C1` = `0x51`, `First_D1` =
`0x61`, `First_E1` = `0x71`, `First_F1` = `0x81`).

**No checksum** exists anywhere in the codebase (no XOR/CRC found).

## Command IDs

### Second generation (characteristic UUID low byte = command ID)

| ID | Class | Meaning |
|---|---|---|
| 0x81 | Second_81 | password (auth) |
| **0x82** | Second_82 | **zone status** (primary STATUS) |
| 0x83 | Second_83 | overall status (enabled, suspend, runAll, zone config) |
| 0x84 | Second_84 | clock |
| 0x85–88 | Second_85–88 | program schedules |
| 0x8A / 0x8F | Second_8a / 8f | active run times (STATUS) |
| 0x90 / 0x91 | Second_90 / 91 | zone names |
| 0x92–9B | Second_92–9B | zone image transfer |
| 0x9C | Second_9C | flow config |
| 0x9D | Second_9D | watering window |
| **0x9E / 0xA3** | Second_9E / A3 | irrigation records (STATUS) |
| **0xA2** | Second_A2 | total irrigation record count (STATUS) |
| 0x9F | Second_9F | debug |
| 0xA0 / 0xA1 | Second_A0 / A1 | start times zones 5–8 |

### First generation (in-payload header byte = command ID)

| Header | Class | Meaning |
|---|---|---|
| 0x51 | First_C1 | password |
| 0x52 | First_C2 | current mode (STATUS) |
| 0x53 | First_C3 | select mode (COMMAND) |
| 0x54 | First_C4 | clock |
| **0x61** | First_D1 | **detail state / zone state** (STATUS) |
| 0x62–66 | First_D2–D6 | duration, schedule, start times, delay |
| 0x67 / 0x68 | First_D7 / D8 | auto / off |
| 0x69 | First_D9 | control + minute (manual run) — COMMAND |
| **0x71** | First_E1 | **detail state / zone state** (STATUS) |
| 0x72–78 | First_E2–E8 | start times, week |
| 0x79 / 0x7A | First_E9 / EA | auto / off |
| 0x7B | First_EB | control + minute (manual run) — COMMAND |
| 0x81 / 0x82 | First_F1 / F2 | empty payload (start/stop commands) |

## Polled vs pushed status

Both mechanisms are supported, but **live status is primarily pushed via GATT
notifications**:

- During service discovery, notification-enabled characteristics are
  subscribed (see [GATT profile](gatt-profile.md)).
- Zone on/off state arrives as unsolicited push on `Second_82` /
  `First_D1` / `First_E1`.
- The app can also **poll** with a query/response GATT read of the same
  characteristic.
- **Battery** is a standard GATT read (`00002a19`) and is polled, not
  notified.

## Status available outside an active GATT connection?

**No.** The app's read and send paths both guard on the connection state and
return early when disconnected. Status (zone state, battery, run times) is only
obtainable over an active GATT connection, either by polling reads or receiving
notifications. The only data persisted across connections is device
identification (address → device record), not status.
