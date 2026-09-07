//! Run only through scripts/test_linux_live.py in an isolated network namespace.
#![cfg(target_os = "linux")]

use ccsds_ethernet_client::{
    Codec, DecodeResult, DecodedMessage, EncodedCommand, Endpoint, LinuxRawEthernetTransport,
    MacAddress, RawEthernetConfig, RawEthernetEndpoint, Session, Transport,
};
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

struct TestCodec;

impl Codec for TestCodec {
    type Command = u8;
    type Acknowledgement = u8;
    type Telemetry = u8;
    type Correlation = u8;
    type Error = std::io::Error;

    fn encode_command(&mut self, command: &u8) -> Result<EncodedCommand<u8>, Self::Error> {
        Ok(EncodedCommand::new(vec![0x10, *command], *command))
    }

    fn decode(
        &mut self,
        payload: &[u8],
        _sender: Endpoint,
    ) -> DecodeResult<u8, u8, u8, Self::Error> {
        match payload {
            [0x20, id] => Ok(DecodedMessage::Acknowledgement {
                acknowledgement: *id,
                correlation: *id,
            }),
            [0x30, id] => Ok(DecodedMessage::Telemetry(*id)),
            _ => Err(std::io::Error::other("invalid test payload")),
        }
    }
}

fn pair(capacity: usize) -> (LinuxRawEthernetTransport, LinuxRawEthernetTransport) {
    assert_eq!(std::env::var("CCSDS_LIVE_TEST").as_deref(), Ok("1"));
    let host = RawEthernetEndpoint::new(
        MacAddress::new([2, 0, 0, 0, 0, 1]),
        Endpoint::new(Ipv4Addr::new(192, 0, 2, 1), 40001).unwrap(),
    )
    .unwrap();
    let peer = RawEthernetEndpoint::new(
        MacAddress::new([2, 0, 0, 0, 0, 2]),
        Endpoint::new(Ipv4Addr::new(192, 0, 2, 2), 40002).unwrap(),
    )
    .unwrap();
    let mut receiver = LinuxRawEthernetTransport::open(
        RawEthernetConfig::new(2, "ccsds-host", host, peer, capacity).unwrap(),
    )
    .unwrap();
    let mut sender = LinuxRawEthernetTransport::open(
        RawEthernetConfig::new(2, "ccsds-peer", peer, host, capacity).unwrap(),
    )
    .unwrap();
    receiver.start_receive().unwrap();
    sender.start_receive().unwrap();
    (receiver, sender)
}

#[test]
#[ignore = "requires isolated Linux veth fixture"]
fn burst_larger_than_ring_preserves_all_packets_including_ack() {
    for capacity in [1, 8] {
        for ack_position in [0, 8, 16] {
            let (mut receiver, mut sender) = pair(capacity);
            for index in 0..17_u8 {
                sender
                    .send(&[if index == ack_position { 0x20 } else { 0x30 }, index])
                    .unwrap();
            }
            for index in 0..17_u8 {
                let packet = receiver
                    .receive(Instant::now() + Duration::from_secs(2))
                    .unwrap();
                assert_eq!(
                    packet.payload,
                    [if index == ack_position { 0x20 } else { 0x30 }, index]
                );
            }
            assert_eq!(receiver.statistics().dropped_frames, 0);
            assert_eq!(receiver.statistics().kernel_dropped_frames, 0);
        }
    }
}

#[test]
#[ignore = "requires isolated Linux veth fixture"]
fn session_preserves_ack_after_overflow_and_rejects_delayed_duplicates() {
    let (receiver, mut peer) = pair(1);
    let remote = Endpoint::new(Ipv4Addr::new(192, 0, 2, 2), 40002).unwrap();
    let mut session = Session::from_transport(TestCodec, receiver, remote).unwrap();
    for id in 0..17 {
        peer.send(&[0x30, id]).unwrap();
    }
    peer.send(&[0x20, 1]).unwrap();
    assert_eq!(
        session
            .exchange_once(&1, Instant::now() + Duration::from_secs(2))
            .unwrap(),
        1
    );
    assert_eq!(session.statistics().dropped_telemetry(), 16);
    assert_eq!(session.statistics().transport().dropped_frames, 0);
    // A duplicate of request 1 precedes the response to request 2.
    peer.send(&[0x20, 1]).unwrap();
    peer.send(&[0x20, 2]).unwrap();
    assert_eq!(
        session
            .exchange_once(&2, Instant::now() + Duration::from_secs(2))
            .unwrap(),
        2
    );
    assert_eq!(
        session
            .statistics()
            .diagnostics()
            .unmatched_acknowledgements,
        1
    );
    // A lost response leaves delivery unknown, without another send.
    let error = session
        .exchange_once(&3, Instant::now() + Duration::from_millis(10))
        .unwrap_err();
    assert!(error.delivery_outcome_unknown());
    assert_eq!(session.statistics().transport().sent_frames, 3);
}

#[test]
#[ignore = "requires isolated Linux veth fixture"]
fn replaced_interface_requires_explicit_reopen() {
    let (mut original, _peer) = pair(1);
    let ip = |args: &[&str]| {
        assert!(
            std::process::Command::new("ip")
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    };
    // Both names belong only to the disposable fixture. Recreate identical
    // addresses to prove that bound index, not just visible identity, matters.
    ip(&["link", "delete", "ccsds-host"]);
    ip(&[
        "link",
        "add",
        "ccsds-host",
        "type",
        "veth",
        "peer",
        "name",
        "ccsds-peer",
    ]);
    for (name, mac) in [
        ("ccsds-host", "02:00:00:00:00:01"),
        ("ccsds-peer", "02:00:00:00:00:02"),
    ] {
        ip(&["link", "set", name, "address", mac, "up"]);
    }
    assert!(
        original
            .start_receive()
            .unwrap_err()
            .to_string()
            .contains("index changed")
    );
    assert!(!original.statistics().receive_ready);
    assert_eq!(
        original.send(&[9]),
        Err(ccsds_ethernet_client::TransportError::NotReady)
    );
    original.close().unwrap();
    let (mut reopened, mut peer) = pair(1);
    peer.send(&[3]).unwrap();
    peer.send(&[1]).unwrap();
    peer.send(&[2]).unwrap();
    for expected in [3, 1, 2] {
        assert_eq!(
            reopened
                .receive(Instant::now() + Duration::from_secs(1))
                .unwrap()
                .payload,
            [expected]
        );
    }
}

#[test]
#[ignore = "requires isolated Linux veth fixture"]
fn measure_virtual_link_exchange_latency() {
    let (mut host, mut peer) = pair(8);
    let mut elapsed = Vec::with_capacity(1000);
    let started = Instant::now();
    for id in 0..1000_u32 {
        let sample = Instant::now();
        let deadline = sample + Duration::from_secs(2);
        host.send(&id.to_be_bytes()).unwrap();
        let command = peer.receive(deadline).unwrap();
        peer.send(&command.payload).unwrap();
        assert_eq!(host.receive(deadline).unwrap().payload, id.to_be_bytes());
        elapsed.push(sample.elapsed().as_nanos());
    }
    let wall_ns = started.elapsed().as_nanos();
    elapsed.sort_unstable();
    println!(
        "{{\"benchmark\":\"virtual_link_exchange\",\"samples\":1000,\"wall_ns\":{wall_ns},\"p50_ns\":{},\"p95_ns\":{},\"p99_ns\":{},\"max_ns\":{},\"kernel_drops\":{}}}",
        elapsed[499],
        elapsed[949],
        elapsed[989],
        elapsed[999],
        host.statistics().kernel_dropped_frames + peer.statistics().kernel_dropped_frames
    );
}
