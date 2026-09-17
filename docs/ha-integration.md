# Hunter BTT — Protocol Features for HA Integration

Normative protocol features identified from the decompiled OEM app
(`com.huiyuan.ble`) that can be exposed in Home Assistant, now that the
connection model is understood.

## GATT profile (2nd gen, service `0000ff80`)

Characteristic UUIDs are `0000ffXX-…`; the low byte is the protocol ID.
`(N)` = notification-enabled. Zone state is at **bytes 10/11** of `ff82` and
is an **enum, not a boolean**: a zone is watering iff the state byte is `5`,
`9` or `17` (`1` = idle/monitoring, `0` = standby, `2` = scheduled reminder).
See [AIS protocol](protocol/ais-protocol.md) for the full decode.

| Char | Protocol | Meaning | HA value |
|---|---|---|---|
| `ff81` | Second_81 | password (auth) | — |
| `ff82` (N) | Second_82 | **zone status** (state, enabled, mode, manual, conflict, suspend) | **Core: zone on/off, conflict, suspend** |
| `ff83` | Second_83 | overall status (enabled, runAll, zone config) | zone enabled state |
| `ff84` | Second_84 | clock | device time |
| `ff85`–`ff88` | Second_85–88 | program schedules (TM/CYC, start/end) | **show scheduled programs** |
| `ff8a`/`ff8f` (N) | Second_8a/8f | active run times | remaining run time |
| `ff90`/`ff91` | Second_90/91 | zone names | **zone naming** |
| `ff92`–`ff9b` | Second_92–9B | zone image transfer | (not needed) |
| `ff9c` | Second_9C | flow config | flow sensor |
| `ff9d` | Second_9D | watering window | — |
| `ff9e`/`ffa3` (N) | Second_9E/A3 | irrigation records (zone, timestamp, run/infiltrate sec) | **watering history** |
| `ffa2` (N) | Second_A2 | total irrigation record count | history count |
| `ff9f` | Second_9F | debug | — |
| `ffa0`/`ffa1` | Second_A0/A1 | start times zones 5–8 | — |

Standard GATT: battery `2a19`, firmware `2a26`, manufacturer `2a29`,
device name `2a00`.

## Recommended HA entities (in priority order)

1. **Zone state** (`ff82` bytes 10/11) — binary sensors for each zone;
   instant via notification. **On iff state ∈ {5, 9, 17}** (idle = 1,
   standby = 0, scheduled reminder = 2 are all off).
2. **Battery** (`2a19`) — sensor.
3. **Conflict** (`ff82` bytes 12/13) — binary sensor / alert when two zones
   conflict.
4. **Suspend watering** (`ff82` byte 1) — binary sensor.
5. **Zone names** (`ff90`/`ff91`) — use for entity naming.
6. **Irrigation records** (`ff9e`/`ffa2`/`ffa3`) — watering history /
   statistics.
7. **Program schedules** (`ff85`–`ff88`) — expose scheduled programs.

## Notes

- Zone state, conflict, and suspend all come from the single `ff82`
  notification — one subscription gives the core live status.
- Irrigation records and schedules require reads of their characteristics;
  they are not needed for the core liveliness use case but add value.
- The byte-offset fix (read bytes 10/11, not 4/8) is required for correct
  zone state.
