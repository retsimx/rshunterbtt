# Hunter BTT Battery Life — Analysis from InfluxDB

Analysis of real-world battery life of the Hunter BTT irrigation controllers,
derived from battery-level data recorded by `rshunterbtt` into the InfluxDB
`sprinkler` bucket.

**Data range:** 2024-01-02 → 2026-09-15 (data current as of analysis).
**Devices:** `front`, `rear`, `small` (2× AA batteries each).

## Method

1. Pulled all battery points from the `sprinkler` bucket (measurement
   `battery`, tag `name`, field `battery`; ~120k points total).
2. Resampled to **daily medians** to suppress noise (the `small` device has
   hourly spikes to 100% that are artifacts).
3. Detected battery **cycles** as sustained jumps back to ~100% (fresh
   batteries installed).
4. Identified **reporting gaps** (>48h with no data) to distinguish:
   - **Battery death** — device goes dark at low battery, later comes back
     with fresh batteries (a swap).
   - **Transient outage** — device dark at high battery, recovers at a
     similar level (not battery-related).

## Key finding: dead-before-swap gaps

Devices frequently **stop reporting (die) before the batteries are
replaced**, so measuring to the swap date overstates battery life. The
dead-before-swap gaps found:

| Device | Cycle end (swap) | Battery died at | Dead before swap |
|---|---|---|---|
| front | 2024-11-28 | 2024-11-20 (12%) | ~7d |
| front | 2025-10-18 | 2025-09-29 (28%) | ~15d |
| rear | 2025-10-18 | 2025-09-29 (0%) | ~15d |
| small | 2024-11-11 | 2024-11-04 (15%) | ~5d |
| small | 2025-10-18 | 2025-09-24 (5%) | ~20d |

Transient outages (not battery deaths) were also seen at high battery:
front Feb 2024, rear Dec 2024 / Feb 2026, small Feb 2024 / Dec 2025 /
Apr 2026 / Mar 2026.

## Battery life per cycle (install → actual battery death)

| Device | Cycle lives (days) |
|---|---|
| front | 323, 305, 303 |
| rear | 298, 337 |
| small (final death) | 307, 299 |
| small (first death) | 216, 228 |

## Results

- **Average battery life: ~9.5–10.2 months** (corrected for dead-before-swap
  time). The uncorrected figure (~10.7 months, measured to swap date) was a
  slight overestimate.
- **Typical range: ~298–337 days** for front and rear.
- **Shortest life observed: ~216 days (~7 months)** — the `small` device's
  first set of batteries (died at 33% on 2024-08-05). Its second set was
  ~228 days (died at 24% on 2025-07-15).
- The **small** device is the outlier: both its early sets died ~1–2 months
  sooner than front/rear. Worth investigating (different hardware, heavier
  usage, or a flakier read path).

## Current status (as of 2026-09-15)

- **front**: dead — went dark 2026-08-17 at 26% (current set, ~303 days).
- **rear**: 0%, still reporting (current set, ~332 days and counting).
- **small**: 4%, still reporting (current set, ~332 days and counting).

## Caveats

- Data is dirty. The `small` device had a noisy Nov-2024 patch (hourly 100%
  spikes) that produced two spurious short "cycles" (2d and 16d); these are
  artifacts and were excluded.
- Devices often stop reporting before 0 (front swapped at 26%, small at
  2–5%), so real life is slightly understated — batteries could go longer.
- The `small` device's low-battery deaths where it *recovered* (33%→dead
  29d→27%, and 24%→dead→2%) make its cycle boundaries ambiguous; this is
  the source of the 216–307d range for that device.

## Implication for the connection-strategy decision

The current `rsHunterBTT` approach (connect-briefly each hour + on-demand
status reads; the connection drops between operations in practice) yields
**~10 months on 2×AA batteries**. This is the **connect-briefly / mostly-
disconnected** pattern — not a persistent connection. That pattern is viable,
but says nothing about the battery cost of a true persistent connection
(needed for notifications), which remains unmeasured.
See [integration strategy](integration-strategy.md) for the full decision analysis.

## Connection interval and latency (2026-09-17)

A controlled test found command latency is dominated by the BLE connection
interval and is a **cliff**, not a linear curve: 60ms → 0.5s, 1000ms → 0.3s,
but 4000ms → ~21s. The deployment was therefore changed from 4000ms to
**1000ms** to keep commands sub-second while still using 16× fewer connection
events than the OEM's 60ms. The battery cost of 1000ms vs 4000ms is unmeasured
— watch the `sprinkler.battery` curve. Full details:
[connection interval](connection-interval.md).
