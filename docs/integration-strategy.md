# Hunter BTT — HA Integration Strategy (Engineering Decision Document)

> **Status: resolved.** The decision gate below was answered **YES** — the
> device holds an idle connection, pushes `ff82` notifications, accepts the
> maximum connection interval, and reconnects cleanly. Architecture **D**
> (persistent connection + notifications) was adopted, and is what the bridge
> implements. This document is kept as the decision record.

## 0. Decision gate (the pivotal question)

**Can this Hunter device maintain an idle BLE connection, given that the host BCM43430A1 controller is a known confounder?**

```
Can Hunter hold an idle GATT connection?
              │
       ┌──────┴──────┐
       NO            YES
       │              │
       ▼              ▼
  Stop here.      Investigate D.
  Use B.          Measure it.
```

There is no point engineering connection intervals for a connection the device refuses to maintain. The test runs on the Pi's BCM43430A1 with `btmon` to identify *who* terminates the connection (device vs host controller). Test 1 collapses the decision tree.

## 1. Established facts

**[OBSERVED — decompiled app]** Status is **GATT-only**. No status in advertisements/beacons; zone state is read/notified over GATT. 2nd-gen service `0000ff80`, zone state on `0000ff82` at payload **bytes 10/11**; battery on `00002a19`. OEM subscribes to notifications on `ff82` (+ others) and sends a 4-byte password on `ff81`.

**[OBSERVED — InfluxDB]** Battery life across front/rear/small: **~9.5–10.2 months** avg; typical 298–337 days; shortest ~216 days (small); devices often go dead 1–3 weeks before a swap.

**[OBSERVED — rsHunterBTT source]** No `disconnect()`; `Peripheral` held in a mutex; every op checks `is_connected()` then `connect()` (full scan→find→connect→discover). Hourly battery poll; no notifications; no connection-parameter control; reads zone state at bytes **4/8** (bug — should be 10/11).

**[OBSERVED — device logs]** `le-connection-abort-by-local`; "BLE not connected, connecting to…" before every op (rear: 1687 connects, all preceded by "not connected"; bedroom: 17958). Bluez config is all-defaults. Devices currently low/dead (not connectable).

**[OBSERVED — host controller]** All three hosts are **Raspberry Pi Zero W Rev 1.1** with the **BCM43430A1** combo chip. dmesg shows repeated `hci0: Opcode 0x2005 failed: -16` (**EBUSY**) during BLE operation, coincident with connection failures. Opcode 0x2005 is **LE Set Random Address** — an HCI operation being rejected as busy because the controller is occupied with another incompatible LE operation.

## 2. Why the connection drops

**[OBSERVED]** The connection is gone by the next operation; the local side reports `le-connection-abort-by-local`; the host controller exhibits repeated **HCI EBUSY** conditions coincident with connection failures.

**[OBSERVED — HCI trace, 2026-09-16]** On front (26% battery), `btmon` shows: connection **established successfully at 60ms interval / 0 latency / 3000ms supervision**, then the device **stops responding** to `LE Read Remote Used Features` (status `Connection Failed to be Established 0x3e`), and the host then sends `Disconnect`. So `le-connection-abort-by-local` is the **host giving up after the device goes silent**, not host controller confusion.

**[HYPOTHESIS]** The device drops the connection shortly after connect — plausibly because a 60ms interval / 0 latency is power-hungry and the device (at 26% battery) can't sustain it. **Confounded by low battery**: cannot yet distinguish "device always drops on connect" from "device too low on battery to sustain a 60ms connection." Retest with fresh batteries.

**[HYPOTHESIS]** Separately, the BCM43430A1/BlueZ controller state may contribute to instability (the EBUSY conditions are host-side). The existing observations still **cannot fully distinguish Hunter-initiated termination from host/controller failure** until tested with fresh batteries on a healthy controller.

## 3. Options

### Option A — Status quo (connect-briefly, on-demand reads, no notifications)
- **[OBSERVED]** ~10 month battery; no liveliness for manual presses; has the byte-offset bug.
- **Pros:** proven, zero work. **Cons:** no liveliness; wrong status bytes.
- **Verdict:** the baseline.

### Option B — Poll and disconnect (scheduled connect→read→disconnect)
- **[OBSERVED]** ~10 month battery under the current operating pattern.
- **Pros:** ~10-month battery life proven under the current operating pattern; simple. **Cons:** liveliness = poll interval; connection churn.
- **Verdict:** the **current empirically validated operating mode** — the only candidate with actual battery-life evidence. (Not "best battery" — that's unmeasured; polling every hour may differ from every six hours, and F could conceivably use less.)

### Option C — Persistent + short interval + notifications
- **[SPEC/INFERENCE]** shortest interval should permit the lowest latency but generally increases the opportunity for connection events / radio activity.
- **Battery:** unknown **[HYP — expected higher consumption]**.
- **[HYPOTHESIS]** possibly unachievable if the device won't hold a connection.
- **Verdict:** likely non-viable.

### Option D — Persistent + long interval + notifications
- **[SPEC]** longer intervals + peripheral latency can reduce radio activity. **[HYPOTHESIS]** that this device accepts a long interval, sleeps between events, only notifies on change, and that battery ≈ today. **None measured.**
- **Verdict:** attractive but entirely unverified and possibly not possible.

### Option E — Hybrid (long-interval connection + notifications, poll fallback)
- **[HYPOTHESIS]** potentially highest robustness **if D works**. The fallback itself introduces complexity and more connection churn, so real-world reliability is not yet known.
- **Verdict:** only worth it if D is proven achievable.

### Option F — On-demand only
- **Pros:** potentially least battery. **Cons:** worst liveliness (manual presses invisible).
- **Verdict:** too weak for your need.

## 4. Comparison

| Option | Battery | Liveliness | Achievable? | Evidence |
|---|---|---|---|---|
| A. Status quo | ~10mo **[OBS]** | none | yes | proven |
| B. Poll & disconnect | ~10mo **[OBS]** | = poll interval | yes | proven |
| C. Short int + notif | unknown **[HYP — expected higher]** | sub-second | **uncertain** | unverified |
| D. Long int + notif | ? **[HYP]** | ~seconds | **uncertain** | unverified |
| E. Hybrid | ? **[HYP]** | ~seconds | **uncertain** | unverified |
| F. On-demand | ? **[HYP]** | none | yes | — |

## 5. Recommendation

**Updated 2026-09-16 after live testing — D is now validated as technically
viable.** The gate (Test 1) passed, and Tests 2A/3/4 confirmed the device
holds an idle connection, pushes `ff82` while idle, accepts the 4000ms max
interval, and recovers via re-subscription. See [architecture](architecture.md).

1. **Fix the byte-offset bug** (read zone state at bytes 10/11) — concrete correctness fix, independent of architecture. **[OBSERVED, confirmed by live status payload]**
2. **Adopt the persistent + notification architecture (D)** — device holds a connection, pushes `ff82` state changes, accepts 4000ms interval. Battery is an out-of-scope non-functional requirement — observe over time in production.
3. **Handle host controller resilience** — the Pi BCM43430A1 gets into a bad state (HCI EBUSY, abort-by-local); a full reboot clears it. Add controller-reset-on-error (prove the causal chain with btmon first).
4. **Re-subscribe on reconnect** — the `ff82` subscription must be recreated after a connection drop (validated).

## 6. What would falsify the recommendation

**D is adopted, but the following remain to confirm:**
- The device sleeps efficiently between 4000ms connection events (sleep current unknown).
- Non-zero peripheral latency is accepted (for even lower power).

**Battery is an out-of-scope non-functional requirement** — observe battery
usage over time in production (existing InfluxDB telemetry) and compare
against the ~10-month connect-briefly baseline, rather than a dedicated A/B
test.

## 7. Epistemic hierarchy

| Claim | Status |
|---|---|
| GATT required for status | **Observed** |
| ~10-month battery life | **Observed** |
| Current implementation reconnects | **Observed** |
| Host (BCM43430A1) reports HCI EBUSY during BLE ops | **Observed** |
| Connection established at 60ms then device goes silent (HCI trace) | **Observed** (front, 26% battery) |
| Idle connection doesn't survive between operations | Observed, **cause unknown** (host vs device vs low battery) |
| Host controller is responsible for connection instability | **Hypothesis** (to test) |
| Device drops connection due to low battery / power-hungry interval | **Hypothesis** (confounded by 26% battery) |
| Hunter deliberately sleeps to terminate BLE | Unknown (weakened by EBUSY finding) |
| Hunter accepts long connection interval | Unknown |
| Hunter sleeps efficiently during long interval | Unknown |
| `ff82` provides useful push state | Observed from OEM behaviour; HA integration still to test |
| Persistent notifications are battery-efficient | Unknown |
| D gives ~10-month battery life | Unknown |
| E is more robust | Unknown |

## 8. Test plan (after batteries are replaced)

**Test 0 — Baseline reproduction.** Run the existing rsHunterBTT behaviour against a freshly powered Hunter and capture one complete cycle (scan → connect → discover → authenticate → read → idle → disconnect) with an HCI trace. This gives a known-good reference for interpreting Test 1, and shows whether the device, BlueZ, the controller (supervision timeout), or the application terminates the connection.

**Test 1 — Idle-connection survival (the gate).** Establish a GATT connection and perform **no application-level GATT operations** for at least 2 hours, or until the device disconnects, while running `btmon`. Record: connection timestamp, negotiated interval, peripheral latency, supervision timeout, every connect/disconnect event, exact disconnect reason, and connection lifetime. Acceptance criterion is explicit: if it dies after 17 minutes you've learned what you need; if it survives 2 hours you've established something useful and can extend the soak if necessary.
```
Connected:       2026-09-20 10:00:01
Interval:        400 ms
Latency:         4
Supervision:     6 s
No application traffic.
Disconnected:    2026-09-20 10:17:43
Reason:          <actual HCI reason>
Initiator:       <central/peripheral>
```

**Test 2 — Notification behaviour.** While connected: subscribe to `ff82`, don't poll, press the physical button, record notification arrival, compare notification payload with an explicit GATT read.

**Test 2A — Notification without polling.** Connect → authenticate → subscribe `ff82` → read initial state → perform **zero GATT reads thereafter** → press physical button → observe `ff82` notification → compare payload → repeat several times. Answers: (a) does Hunter push state changes? (b) does it push while otherwise idle? If yes, the basic D architecture is technically viable.

**Test 3 — Connection parameters.** Request the proposed long interval and record what Hunter actually accepts/rejects and the resulting **negotiated** values — connection interval, peripheral latency, and supervision timeout observed from the controller/HCI trace (not merely `hcitool leinfo`). Record what the device accepted → X/Y/Z, not merely that a request was made.

**Test 4 — Reconnection.** Force a disconnect (kill RF / move out of range). Record as a concrete pass/fail whether the device sends the expected `ff82` notification after reconnect **without a fresh subscription**, whether the subscription must be recreated, whether the device accepts reconnection, and recovery time.

**Test 5 — Battery (out of scope).** No dedicated A/B test. Observe battery usage over time in production (existing InfluxDB telemetry) and compare against the ~10-month connect-briefly baseline.

**Dependency chain (no premature optimisation):** 1 idle survival → 2 notification behaviour → 2A notification-without-polling → 3 connection parameters → 4 reconnection → 5 battery.

**On the proposed controller reset:** `hciconfig hci0 reset` (or a bluez power-cycle) is a plausible mitigation, but **do not put it into production code yet**. First prove the causal chain with `btmon`: connection attempt → controller enters bad state → 0x2005 EBUSY → reset → controller healthy → connection succeeds. Only then add it to the reconnect path.

---

This is defensible: the ~10-month figure is used only for what it supports (the connect-briefly approach is viable), and the remaining uncertainty is concentrated on the single empirical question — can this Hunter sustain an idle connection, and under what parameters? The next useful action is getting fresh batteries into a Hunter and running Test 1 with `btmon`.
