# BLE connection interval: latency, the peripheral's re-negotiation, and diagnostics

**Status**: deployed (2026-09-17) — `CONN_INTERVAL_MS=1000` on all 3 hosts,
with a process-lifetime HCI diagnostic monitor.
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
   actually running at, so the bridge requests it at setup and keeps observing
   negotiated interval/controller events diagnostically.

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

## Runtime behavior

The bridge requests the configured interval during connection setup. The
peripheral may subsequently re-negotiate that interval, so the effective link
latency may differ from the requested value.

### HCI diagnostic monitor (`hci_monitor`)

The bridge opens an `HCI_CHANNEL_MONITOR` socket and decodes
`LE Connection Update Complete`, disconnection, and selected controller command
status events. It logs controller status (including `0x2005`) but never sends
an HCI re-apply command and never uses HCI status to gate or restart recovery.

**Kernel encoding (verified against source, not memory):**
- Channel enum is **not stable across trees**. This Raspberry Pi 6.12.y kernel
  uses `RAW=0, USER=1, MONITOR=2, CONTROL=3, LOGGING=4`; mainline uses
  `MONITOR=3, CONTROL=2`. Binding the wrong value either fails (`USER` →
  `EINVAL`) or silently delivers mgmt events instead of monitor frames.
- Monitor packet types (`include/net/bluetooth/hci_mon.h`):
  `COMMAND_PKT=2, EVENT_PKT=3, ACL_TX=4, ACL_RX=5` (18/19 are ISO).

### Observed behavior (fresh reboots, all 3 hosts)

The initial request and negotiated interval can differ. Capture event lines for
diagnostic evidence; do not interpret HCI monitoring as a corrective recovery
loop.

```
HCI monitor active: diagnostic controller logging (target interval 1000ms)
HCI connection update complete: handle 64 interval 48ms status 0x00
```

Effective latency remains a property of the negotiated peripheral/BlueZ link,
not a command-driven re-apply performed by `hci_monitor`.

## Configuration

- **`CONN_INTERVAL_MS`** (default `4000`, BLE range 8–4000). The bridge requests
  it (with retry) during connection setup. The HCI monitor only records resulting
  negotiated/controller events; it does not re-assert the interval.

## Deployment decision

**1000 ms** on all 3 hosts. It gives ~5.2–5.7 s command latency at **4× fewer
connection events than 4000 ms and 16× fewer than the OEM's 60 ms**. The
peripheral may still re-negotiate toward ~48–60 ms, and the monitor records the
actual link for diagnostics. If battery regresses, raise the requested interval;
if 5 s is too slow, lower it.
The HA `valve` entities remain `optimistic` so the UI is instant regardless.

## Open items

- Battery: observe `sprinkler.battery` over the coming weeks at 1000 ms.
- The HCI monitor decodes `LE Connection Update Complete` events for diagnostics
  and does not filter or gate recovery by connection handle.
- Channel enum is kernel-specific (see above); a kernel change on these hosts
  would require updating `HCI_CHANNEL_MONITOR`.
