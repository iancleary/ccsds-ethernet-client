# Agent operating loop

This repository is easiest to maintain when future agents treat it as a small
crate with a deliberately narrow protocol boundary. The useful accretion path
is to make the CCSDS/Ethernet exchange contract easier to verify, not to grow a
framework around it.

## System map

- `src/ethernet.rs` owns strict raw Ethernet, IPv4, and UDP frame construction,
  parsing, config validation, and schema-version behavior.
- `src/session.rs` owns command exchange lifecycle, correlation, telemetry
  buffering, close semantics, and delivery-outcome error boundaries.
- `src/transport.rs` owns the transport trait, hardware-free memory transport,
  packet-ring accounting, and portable test doubles.
- `src/linux.rs` is the only Linux raw-socket implementation and the only
  unsafe/system-call boundary. It must stay behind `cfg(target_os = "linux")`.
- `tests/frame_transport.rs` is the executable frame/config/ring contract.
- `tests/session.rs` is the executable session/codec/error contract.
- `docs/release.md`, `scripts/cut_release.py`, and
  `scripts/test_cut_release.py` own release behavior. Do not describe a release
  step elsewhere unless it routes back to those files.

## Invariants

Preserve these unless the pull request explicitly changes the public contract:

- The crate is transport and exchange mechanics only. Mission APIDs, packet
  schemas, request-ID allocation, acknowledgement status meaning, retries,
  recording schemas, actuation logic, decisions, intent, and safety policy
  belong to consuming systems.
- `RawEthernetConfig` schema version `1` is strict: concrete unicast IPv4
  endpoints, nonzero UDP ports, unicast nonzero MAC addresses, distinct
  host/board identity, valid Linux interface name, and nonzero packet-ring
  capacity.
- Standard Ethernet MTU is the v1 payload limit. UDP payloads above
  `STANDARD_MTU_UDP_PAYLOAD_BYTES` are rejected rather than fragmented.
- The Linux transport uses an interface-bound `AF_PACKET` raw socket. Live use
  requires Linux and deployment-managed `CAP_NET_RAW`; default tests must stay
  hardware-free and root-free.
- `Session::from_transport` starts receive before any send can happen.
  `exchange_once` sends at most once, uses an absolute monotonic deadline, and
  distinguishes an expired pre-send deadline from a post-send unknown delivery
  outcome.
- Matching acknowledgements are returned as typed codec values, including
  statuses a mission may consider busy or rejected. This crate must not turn
  mission acknowledgement policy into generic success or failure policy.
- `next_telemetry` returns telemetry observed while waiting for an
  acknowledgement. Queue capacity and drops are observable through statistics.
- `close` is idempotent and leaves the session closed even if the underlying
  transport reports a close error.

## Change loop

1. Read the nearest contract first: `README.md`, this file, and the tests that
   cover the module being changed.
2. State the boundary being changed in the pull request: frame/config,
   session/exchange, Linux transport, release process, or documentation only.
3. Add or update executable evidence beside the behavior:
   `tests/frame_transport.rs` for bytes and config validation,
   `tests/session.rs` for codec/session semantics,
   `scripts/test_cut_release.py` for release-runner behavior.
4. Keep consuming-system responsibilities out of the crate. Prefer typed hooks
   and explicit errors over policy defaults.
5. Update the README contract only when public behavior changes. Update this
   operating loop when agents would otherwise need to rediscover a durable
   maintenance rule.
6. Run the lightest check that proves the change. At minimum run
   `git diff --check -- .`; for code, tests, release scripts, or public API
   changes run `just check`.

## Review prompts

Before opening a PR, answer these locally:

- Does the change keep live raw-socket behavior isolated from default tests?
- Does a post-send failure still surface as unknown delivery outcome when the
  command may have left the process?
- Did a generic helper accidentally encode mission policy?
- Is new documentation describing current behavior rather than a desired future
  design?
- If release behavior changed, do `docs/release.md` and the checked-in runner
  still agree?
