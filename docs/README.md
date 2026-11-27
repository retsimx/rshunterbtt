# Documentation

Reference notes on the Hunter BTT BLE protocol and on the design of
`rshunterbtt`, decoded from the official Android app (Hunter BTT 6.1.7) and
from live probing of the hardware. These are the working notes behind the
bridge: the protocol as understood, the GATT profile, the empirical findings
that shaped the design, and the parts of the protocol the bridge does not
implement.

## Protocol

- **[Advertising vs GATT](protocol/advertising-vs-gatt.md)** — does the device
  broadcast its state, or is it GATT-only? (Answer: GATT-only.)
- **[GATT profile](protocol/gatt-profile.md)** — services, characteristics, and
  the notification setup.
- **[AIS protocol](protocol/ais-protocol.md)** — the application framing,
  command IDs, and the 14-byte status payload, including the zone-state enum.

## Design

- **[Architecture](architecture.md)** — the persistent-connection,
  notification-driven design and the findings that motivated it.
- **[Connection interval](connection-interval.md)** — latency versus battery,
  the peripheral's re-negotiation, and the interval guard.
- **[Integration strategy](integration-strategy.md)** — the options considered
  for home-automation integration and the decision record.

## Operations

- **[Battery life](battery-life.md)** — real-world battery life from InfluxDB.
- **[Protocol coverage and gaps](gaps.md)** — what is implemented and what is not.
- **[HA integration](ha-integration.md)** — the protocol features available to a
  home-automation consumer.

## Source material and method

The protocol was decoded from the decompiled OEM app and validated against live
BLE probes of the hardware (HCI captures via `btmon`, plus a purpose-built
probe). Every claim in these notes is grounded in one of those two sources;
where a behaviour was observed only once, or remains unverified, it is marked
as such in place.
