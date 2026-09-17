# BLE connection interval: latency, the peripheral's re-negotiation, and enforcement

**Status**: deployed (2026-09-17) — `CONN_INTERVAL_MS=1000` on all 3 hosts,
with a connect/disconnect/connect dance and an event-driven interval guard.
**Scope**: `rshunterbtt` (`src/hci.rs`, `src/hci_monitor.rs`, `src/lib.rs`).

Relates to: [architecture](architecture.md) (persistent connection model),
[battery life](battery-life.md), [integration strategy](integration-strategy.md), rshunterbtt Epic H.

## Summary

The bridge holds a persistent BLE connection and requests a connection
interval via a pure-Rust HCI `LE Connection Update`. The interval determines
command latency (fewer connection events = slower), so it is a battery-vs-latency
trade-off. Two things make it non-trivial, both established empirically:

1. **The Hunter BTT peripheral actively re-negotiates the interval back to
   ~48–60 ms** (L2CAP Connection Parameter Update Request) shortly after
   connecting, and intermittently later. BlueZ honours the peripheral, silently
   reverting the bridge's requested interval.
2. Because of (1), the requested interval is often **not** what the link is
   actually running at, so the bridge must both re-apply it and keep watching.

## Empirical latency data (2026-09-17, stop command = 3 GATT ops)

Command latency tracks the **effective** interval, not the requested one:

| Interval | Method | Latency |
|---|---|---|
| 60 ms | `hcitool lecup` (applied) | 0.50 s |
| 48–60 ms | peripheral's own default (unapplied case) | 0.26–0.30 s |
| 1000 ms | applied | ~5.2–5.7 s |
| 2000 ms | applied | ~12 s |
| 4000 ms | applied | ~21–24 s |

A clean linear-ish fit for the applied cases is **latency ≈ 6 × interval**
(3 GATT operations, ~2 connection events each). Confirmed by a phase sweep:
at an applied 4000 ms, per-phase latencies spread 0–4 s; at a (silently) fast
link the same sweep returns a flat ~0.28 s regardless of phase.

### The unapplied-interval trap

`btmon` captured the exact sequence on a fresh connection:

```
LE Connection Complete          -> 48.75 ms
LE Connection Update Complete   -> 195 ms      (BlueZ / peripheral preference)
LE Connection Update Complete   -> 1000 ms     (bridge's request)
LE L2CAP: Connection Parameter Update Request  (peripheral asks for fast)
LE Connection Update Complete   -> 48.75 ms    (BlueZ honours it -> revert)
```

So a `negotiated 1000ms` log line can be immediately followed by a silent
revert. Latency — not the log line — is the source of truth.

## The fix

### 1. connect → disconnect → connect dance

The peripheral sends the L2CAP request on its **first** connection after boot;
on a **reconnect** it does not. `run_connection_setup` therefore connects,
disconnects, and reconnects, then applies the interval. `btmon` shows only
BlueZ's own 195 ms step and then the bridge's target on the second connection,
with no peripheral request.

### 2. Event-driven interval guard (`hci_monitor`)

The bridge opens an `HCI_CHANNEL_MONITOR` socket, decodes
`LE Connection Update Complete` events, and whenever the reported interval is
off-target it re-applies the configured interval (rate-limited to 1 per 3 s,
on its own thread so the monitor loop never stalls).

**Kernel encoding (verified against source, not memory):**
- Channel enum is **not stable across trees**. This Raspberry Pi 6.12.y kernel
  uses `RAW=0, USER=1, MONITOR=2, CONTROL=3, LOGGING=4`; mainline uses
  `MONITOR=3, CONTROL=2`. Binding the wrong value either fails (`USER` →
  `EINVAL`) or silently delivers mgmt events instead of monitor frames.
- Monitor packet types (`include/net/bluetooth/hci_mon.h`):
  `COMMAND_PKT=2, EVENT_PKT=3, ACL_TX=4, ACL_RX=5` (18/19 are ISO).

### Proven behaviour (fresh reboots, all 3 hosts)

```
HCI monitor active: interval guard watching for re-negotiation (target 1000ms)
interval changed to 60ms (target 1000ms); re-applying
connection interval updated: requested 1000ms, negotiated 1000ms
```

| Host | Reverts detected → corrected | Effective latency |
|---|---|---|
| host A | 60 ms, 50 ms | 5.52–5.54 s |
| host B | 198 ms, 60 ms | (1000 ms held) |
| host C | 200 ms | (1000 ms held) |

Corrections settle (typically 1–2 per connection); no ping-pong observed.

## Configuration

- **`CONN_INTERVAL_MS`** (default `4000`, BLE range 8–4000). The bridge requests
  it (with retry) and the guard re-asserts it on every revert.
- **`/etc/bluetooth/main.conf`** `[LE] Min/MaxConnectionInterval` — the fallback
  when the runtime request fails. Units are 1.25 ms: use `interval_ms × 0.8`
  (`1000 ms → 800`). Keep it in sync with `CONN_INTERVAL_MS`.

## Deployment decision

**1000 ms** on all 3 hosts. It gives ~5.2–5.7 s command latency at **4× fewer
connection events than 4000 ms and 16× fewer than the OEM's 60 ms**. The
peripheral actively wants ~48–60 ms; the guard holds 1000 ms against it, so
the battery benefit is real (not silently lost to a revert). If battery
regresses, raise the interval (and `main.conf`); if 5 s is too slow, lower it.
The HA `valve` entities remain `optimistic` so the UI is instant regardless.

## Open items

- Battery: observe `sprinkler.battery` over the coming weeks at 1000 ms.
- The guard only decodes `LE Connection Update Complete`; it does not filter by
  handle, so a *different* connected device's parameter change can trigger a
  (harmless, idempotent) re-apply of the configured interval.
- Channel enum is kernel-specific (see above); a kernel change on these hosts
  would require updating `HCI_CHANNEL_MONITOR`.
