# Agent operating loop

This repository is easiest to maintain when future agents treat it as a small
crate with a deliberately narrow protocol boundary. The useful accretion path
is to make the CCSDS/Ethernet exchange contract easier to verify, not to grow a
framework around it.

## System map

- `src/ethernet.rs` owns strict raw Ethernet, IPv4, IPv6, and UDP frame
  construction, parsing, config validation, and schema-version behavior.
- `src/session.rs` owns command exchange lifecycle, correlation, telemetry
  buffering, close semantics, and delivery-outcome error boundaries.
- `src/transport.rs` owns the transport trait, hardware-free memory transport,
  packet-ring accounting, observation statistics, and portable test doubles.
- `src/linux.rs` is the only Linux raw-socket implementation and the only
  unsafe/system-call boundary. It must stay behind `cfg(target_os = "linux")`.
- `src/python.rs`, `pyproject.toml`, and `python/` own the thin byte-oriented
  Python binding. They must not duplicate transport or mission policy.
- `tests/frame_transport.rs` is the executable frame/config/ring contract.
- `tests/session.rs` is the executable session/codec/error contract.
- `python/tests/test_bindings.py` is the hardware-free Python boundary
  contract.
- `docs/release.md`, `scripts/cut_release.py`, and
  `scripts/test_cut_release.py` own local release behavior. The release
  workflow and `scripts/test_release_workflow.py` own registry publication
  order. Do not describe a release step elsewhere unless it routes back to
  those files.

## Invariants

Preserve these unless the pull request explicitly changes the public contract:

- The crate is transport and exchange mechanics only. Mission APIDs, packet
  schemas, request-ID allocation, acknowledgement status meaning, retries,
  recording schemas, actuation logic, decisions, intent, and safety policy
  belong to consuming systems.
- Consumers supply every local and peer MAC address, IP address, UDP port,
  interface name, deadline, codec, recording schema, and operational policy.
- `RawEthernetConfig` schema version `2` is strict: concrete unicast IPv4 or
  IPv6 endpoints, matching IP address families, nonzero UDP ports, unicast
  nonzero MAC addresses, distinct host/board identity, valid Linux interface
  name, and nonzero packet-ring capacity.
- The crate must not infer, discover, select, rewrite, or learn peer addresses.
- Standard Ethernet MTU is the configured-family payload limit. IPv4 UDP
  payloads above `STANDARD_MTU_IPV4_UDP_PAYLOAD_BYTES` and IPv6 UDP payloads
  above `STANDARD_MTU_IPV6_UDP_PAYLOAD_BYTES` are rejected rather than
  fragmented.
- Linux receive statistics are observations only. Detailed counters do not
  imply that a consumer should accept, reject, persist, invalidate, or ignore a
  frame for mission purposes. Ignored payload bytes are not retained.
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
- Python accepts caller-owned addresses and byte payloads only. It does not
  add schemas, correlation, acknowledgement interpretation, retry, recording,
  or safety policy. Blocking live calls detach from the Python interpreter.
- Release artifacts are built before publication. crates.io publication must
  succeed before PyPI Trusted Publishing starts.

## Change loop

1. Read the nearest contract first: `README.md`, this file, and the tests that
   cover the module being changed.
2. State the boundary being changed in the pull request: frame/config,
   session/exchange, Linux transport, release process, or documentation only.
3. Add or update executable evidence beside the behavior:
   `tests/frame_transport.rs` for bytes and config validation,
   `tests/session.rs` for codec/session semantics,
   `python/tests/test_bindings.py` for the Python boundary,
   `scripts/test_release_workflow.py` for registry publication ordering, and
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
- Does the crate still avoid selecting or learning caller-owned addresses?
- Does a post-send failure still surface as unknown delivery outcome when the
  command may have left the process?
- Did a generic helper accidentally encode mission policy?
- Is new documentation describing current behavior rather than a desired future
  design?
- If release behavior changed, do `docs/release.md` and the checked-in runner
  still agree?
