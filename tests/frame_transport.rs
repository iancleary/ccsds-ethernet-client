#[rustfmt::skip]
use std::net::{
    Ipv4Addr,
    Ipv6Addr,
};
use std::str::FromStr;

#[rustfmt::skip]
#[allow(deprecated)]
use ccsds_ethernet_client::{
    BoundedPacketRing,
    Endpoint,
    EthernetError,
    FrameDisposition,
    IpPacketOptions,
    MacAddress,
    RAW_ETHERNET_CONFIG_SCHEMA_VERSION,
    RawEthernetConfig,
    RawEthernetEndpoint,
    STANDARD_MTU_IPV4_UDP_PAYLOAD_BYTES,
    STANDARD_MTU_IPV6_UDP_PAYLOAD_BYTES,
    STANDARD_MTU_UDP_PAYLOAD_BYTES,
    build_udp_frame,
    parse_udp_frame,
};

const BOARD_IP: Ipv4Addr = Ipv4Addr::new(169, 254, 209, 0);
const HOST_IP: Ipv4Addr = Ipv4Addr::new(169, 254, 209, 1);
const BOARD_IPV6: Ipv6Addr = Ipv6Addr::new(0x2001, 0x0db8, 0x1234, 0, 0, 0, 0, 0x7a);
const HOST_IPV6: Ipv6Addr = Ipv6Addr::new(0x2001, 0x0db8, 0x1234, 0, 0, 0, 0, 1);
const BOARD_PORT: u16 = 24_576;
const HOST_PORT: u16 = 49_152;

fn raw_endpoint(mac: &str, ipv4: Ipv4Addr, port: u16) -> RawEthernetEndpoint {
    RawEthernetEndpoint::new(
        MacAddress::from_str(mac).expect("MAC"),
        Endpoint::new(ipv4, port).expect("endpoint"),
    )
    .expect("raw endpoint")
}

fn raw_ipv6_endpoint(mac: &str, ipv6: Ipv6Addr, port: u16) -> RawEthernetEndpoint {
    RawEthernetEndpoint::new(
        MacAddress::from_str(mac).expect("MAC"),
        Endpoint::new(ipv6, port).expect("endpoint"),
    )
    .expect("raw endpoint")
}

fn config() -> RawEthernetConfig {
    RawEthernetConfig::new(
        RAW_ETHERNET_CONFIG_SCHEMA_VERSION,
        "eth0",
        raw_endpoint("02:00:00:00:00:01", HOST_IP, HOST_PORT),
        raw_endpoint("02:00:00:00:00:7a", BOARD_IP, BOARD_PORT),
        8,
    )
    .expect("config")
}

fn ipv6_config() -> RawEthernetConfig {
    RawEthernetConfig::new(
        RAW_ETHERNET_CONFIG_SCHEMA_VERSION,
        "eth0",
        raw_ipv6_endpoint("02:00:00:00:00:01", HOST_IPV6, HOST_PORT),
        raw_ipv6_endpoint("02:00:00:00:00:7a", BOARD_IPV6, BOARD_PORT),
        8,
    )
    .expect("IPv6 config")
}

fn inbound_frame_from(
    sender: RawEthernetEndpoint,
    recipient: RawEthernetEndpoint,
    payload: &[u8],
) -> Vec<u8> {
    let reverse = RawEthernetConfig::new(
        RAW_ETHERNET_CONFIG_SCHEMA_VERSION,
        "eth0",
        sender,
        recipient,
        8,
    )
    .expect("reverse config");
    build_udp_frame(
        &reverse,
        payload,
        IpPacketOptions::Ipv4 { identification: 17 },
    )
    .expect("build inbound frame")
}

fn inbound_frame(payload: &[u8]) -> Vec<u8> {
    let expected = config();
    inbound_frame_from(expected.board(), expected.host(), payload)
}

fn checksum(bytes: &[u8]) -> u16 {
    let mut sum = bytes.chunks(2).fold(0_u32, |sum, chunk| {
        sum + u32::from(if chunk.len() == 2 {
            u16::from_be_bytes([chunk[0], chunk[1]])
        } else {
            u16::from(chunk[0]) << 8
        })
    });
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

fn set_ipv4_checksum(frame: &mut [u8]) {
    frame[24..26].fill(0);
    let value = checksum(&frame[14..34]);
    frame[24..26].copy_from_slice(&value.to_be_bytes());
}

fn set_udp_checksum(frame: &mut [u8]) {
    let udp_length = usize::from(u16::from_be_bytes([frame[38], frame[39]]));
    frame[40..42].fill(0);
    let mut pseudo_header = Vec::with_capacity(12 + udp_length);
    pseudo_header.extend_from_slice(&frame[26..34]);
    pseudo_header.extend_from_slice(&[0, 17]);
    pseudo_header.extend_from_slice(&(udp_length as u16).to_be_bytes());
    pseudo_header.extend_from_slice(&frame[34..34 + udp_length]);
    let value = checksum(&pseudo_header);
    frame[40..42].copy_from_slice(&if value == 0 { 0xffff } else { value }.to_be_bytes());
}

// Independent oracle: concatenate the on-wire pseudo-header and UDP bytes.
// This deliberately retains the allocation that production no longer needs.
fn reference_udp_checksum(frame: &[u8], ipv6: bool) -> u16 {
    let udp = if ipv6 { 54 } else { 34 };
    let length = u16::from_be_bytes([frame[udp + 4], frame[udp + 5]]);
    let mut pseudo = Vec::new();
    if ipv6 {
        pseudo.extend_from_slice(&frame[22..54]);
        pseudo.extend_from_slice(&u32::from(length).to_be_bytes());
        pseudo.extend_from_slice(&[0, 0, 0, 17]);
    } else {
        pseudo.extend_from_slice(&frame[26..34]);
        pseudo.extend_from_slice(&[0, 17]);
        pseudo.extend_from_slice(&length.to_be_bytes());
    }
    let header_length = pseudo.len();
    pseudo.extend_from_slice(&frame[udp..udp + usize::from(length)]);
    pseudo[header_length + 6..header_length + 8].fill(0);
    checksum(&pseudo)
}

#[test]
fn checksums_match_concatenated_oracle_for_every_supported_payload_length() {
    use ccsds_ethernet_client::Ipv4ChecksumPolicy;

    for (selected, options, ipv6) in [
        (config(), IpPacketOptions::Ipv4 { identification: 7 }, false),
        (ipv6_config(), IpPacketOptions::Ipv6, true),
    ] {
        let selected = selected.with_ipv4_checksum_policy(Ipv4ChecksumPolicy::Require);
        let reverse = RawEthernetConfig::new(2, "eth0", selected.board(), selected.host(), 8)
            .unwrap()
            .with_ipv4_checksum_policy(Ipv4ChecksumPolicy::Generate);
        let udp = if ipv6 { 54 } else { 34 };
        for length in 0..=selected.maximum_udp_payload_bytes() {
            let payload: Vec<_> = (0..length)
                .map(|i| (i.wrapping_mul(197) ^ length) as u8)
                .collect();
            let mut frame = build_udp_frame(&reverse, &payload, options).unwrap();
            let expected = reference_udp_checksum(&frame, ipv6);
            let expected = if expected == 0 { 0xffff } else { expected };
            assert_eq!(
                &frame[udp + 6..udp + 8],
                &expected.to_be_bytes(),
                "IPv6={ipv6}, length={length}"
            );
            // Ethernet padding is outside the checksum and the returned payload.
            frame[udp + 8 + length..].fill(0xa5);
            assert_eq!(
                parse_udp_frame(&frame, &selected),
                FrameDisposition::Matched {
                    payload,
                    sender: selected.board().network(),
                }
            );
            if length > 0 {
                frame[udp + 8 + length - 1] ^= 1;
                let error = if ipv6 {
                    EthernetError::InvalidIpv6UdpChecksum
                } else {
                    EthernetError::InvalidUdpChecksum
                };
                assert_eq!(
                    parse_udp_frame(&frame, &selected),
                    FrameDisposition::Invalid(error)
                );
            }
        }
    }
}

#[test]
fn computed_zero_udp_checksum_is_transmitted_as_all_ones() {
    use ccsds_ethernet_client::Ipv4ChecksumPolicy;

    for (selected, options, ipv6) in [
        (config(), IpPacketOptions::Ipv4 { identification: 7 }, false),
        (ipv6_config(), IpPacketOptions::Ipv6, true),
    ] {
        let selected = selected.with_ipv4_checksum_policy(Ipv4ChecksumPolicy::Require);
        let reverse = RawEthernetConfig::new(2, "eth0", selected.board(), selected.host(), 8)
            .unwrap()
            .with_ipv4_checksum_policy(Ipv4ChecksumPolicy::Generate);
        let initial = build_udp_frame(&reverse, &[0, 0], options).unwrap();
        let payload = reference_udp_checksum(&initial, ipv6).to_be_bytes();
        let frame = build_udp_frame(&reverse, &payload, options).unwrap();
        let udp = if ipv6 { 54 } else { 34 };
        assert_eq!(reference_udp_checksum(&frame, ipv6), 0);
        assert_eq!(&frame[udp + 6..udp + 8], &[0xff, 0xff]);
        assert_eq!(
            parse_udp_frame(&frame, &selected),
            FrameDisposition::Matched {
                payload: payload.to_vec(),
                sender: selected.board().network(),
            }
        );
    }
}

#[test]
fn config_rejects_ambiguous_or_unsupported_identity() {
    assert!(MacAddress::from_str("02:00:00:00:00:7a").is_ok());
    assert!(MacAddress::from_str("ff:ff:ff:ff:ff:ff").is_err());
    assert!(MacAddress::from_str("00:00:00:00:00:00").is_err());
    assert!(Endpoint::new(Ipv4Addr::UNSPECIFIED, BOARD_PORT).is_err());
    assert!(Endpoint::new(Ipv6Addr::UNSPECIFIED, BOARD_PORT).is_err());
    assert!(Endpoint::new(Ipv6Addr::LOCALHOST, BOARD_PORT).is_err());
    assert!(Endpoint::new(BOARD_IP, 0).is_err());
    assert_eq!(
        Endpoint::new(HOST_IPV6, HOST_PORT)
            .expect("IPv6 endpoint")
            .as_ipv6(),
        Some(HOST_IPV6)
    );

    let host = raw_endpoint("02:00:00:00:00:01", HOST_IP, HOST_PORT);
    let board = raw_endpoint("02:00:00:00:00:7a", BOARD_IP, BOARD_PORT);
    assert!(matches!(
        RawEthernetConfig::new(1, "eth0", host, board, 8),
        Err(EthernetError::UnsupportedSchemaVersion(1))
    ));
    assert!(RawEthernetConfig::new(2, "../eth0", host, board, 8).is_err());
    assert!(RawEthernetConfig::new(2, "eth0", host, host, 8).is_err());
    assert!(RawEthernetConfig::new(2, "eth0", host, board, 0).is_err());
    assert!(matches!(
        RawEthernetConfig::new(
            2,
            "eth0",
            host,
            raw_ipv6_endpoint("02:00:00:00:00:7a", BOARD_IPV6, BOARD_PORT),
            8,
        ),
        Err(EthernetError::MixedIpFamilies { .. })
    ));
}

#[test]
fn exact_udp_frame_round_trip_is_strict() {
    let payload = (0_u8..54).collect::<Vec<_>>();
    let frame = inbound_frame(&payload);
    assert_eq!(frame.len(), 96);
    assert_eq!(
        parse_udp_frame(&frame, &config()),
        FrameDisposition::Matched {
            payload: payload.clone(),
            sender: Endpoint::new(BOARD_IP, BOARD_PORT).expect("board endpoint"),
        }
    );

    let mut foreign_destination_mac = frame.clone();
    foreign_destination_mac[0] ^= 0x10;
    assert_eq!(
        parse_udp_frame(&foreign_destination_mac, &config()),
        FrameDisposition::Foreign
    );
    let mut foreign_source_mac = frame.clone();
    foreign_source_mac[6] ^= 0x10;
    assert_eq!(
        parse_udp_frame(&foreign_source_mac, &config()),
        FrameDisposition::Foreign
    );

    let selected = config();
    let foreign_source_ip = inbound_frame_from(
        RawEthernetEndpoint::new(
            selected.board().mac(),
            Endpoint::new(Ipv4Addr::new(169, 254, 209, 2), BOARD_PORT).expect("endpoint"),
        )
        .expect("raw endpoint"),
        selected.host(),
        &payload,
    );
    assert_eq!(
        parse_udp_frame(&foreign_source_ip, &selected),
        FrameDisposition::Foreign
    );
    let foreign_destination_ip = inbound_frame_from(
        selected.board(),
        RawEthernetEndpoint::new(
            selected.host().mac(),
            Endpoint::new(Ipv4Addr::new(169, 254, 209, 3), HOST_PORT).expect("endpoint"),
        )
        .expect("raw endpoint"),
        &payload,
    );
    assert_eq!(
        parse_udp_frame(&foreign_destination_ip, &selected),
        FrameDisposition::Foreign
    );
    let foreign_source_port = inbound_frame_from(
        RawEthernetEndpoint::new(
            selected.board().mac(),
            Endpoint::new(BOARD_IP, BOARD_PORT + 1).expect("endpoint"),
        )
        .expect("raw endpoint"),
        selected.host(),
        &payload,
    );
    assert_eq!(
        parse_udp_frame(&foreign_source_port, &selected),
        FrameDisposition::Foreign
    );
    let foreign_destination_port = inbound_frame_from(
        selected.board(),
        RawEthernetEndpoint::new(
            selected.host().mac(),
            Endpoint::new(HOST_IP, HOST_PORT + 1).expect("endpoint"),
        )
        .expect("raw endpoint"),
        &payload,
    );
    assert_eq!(
        parse_udp_frame(&foreign_destination_port, &selected),
        FrameDisposition::Foreign
    );

    let mut bad_ip_checksum = frame.clone();
    bad_ip_checksum[24] ^= 1;
    assert_eq!(
        parse_udp_frame(&bad_ip_checksum, &config()),
        FrameDisposition::Invalid(EthernetError::InvalidIpv4Checksum)
    );

    let mut bad_udp_checksum = frame;
    bad_udp_checksum[40..42].copy_from_slice(&1_u16.to_be_bytes());
    assert_eq!(
        parse_udp_frame(&bad_udp_checksum, &config()),
        FrameDisposition::Invalid(EthernetError::InvalidUdpChecksum)
    );
}

#[test]
fn short_frames_are_padded_without_changing_protocol_lengths() {
    let frame = build_udp_frame(
        &config(),
        &[1, 2, 3],
        IpPacketOptions::Ipv4 {
            identification: 0x1234,
        },
    )
    .expect("build frame");
    assert_eq!(frame.len(), 60);
    assert_eq!(u16::from_be_bytes([frame[16], frame[17]]), 31);
    assert_eq!(u16::from_be_bytes([frame[38], frame[39]]), 11);
    assert_eq!(&frame[42..45], &[1, 2, 3]);
    assert!(frame[45..].iter().all(|byte| *byte == 0));
}

#[test]
#[allow(deprecated)]
fn standard_mtu_payload_ceiling_is_enforced() {
    let payload = vec![0x5a; STANDARD_MTU_IPV4_UDP_PAYLOAD_BYTES];
    let frame = build_udp_frame(
        &config(),
        &payload,
        IpPacketOptions::Ipv4 { identification: 1 },
    )
    .expect("maximum payload");
    assert_eq!(frame.len(), 1514);
    assert_eq!(&frame[40..42], &[0, 0]);
    assert_eq!(
        STANDARD_MTU_UDP_PAYLOAD_BYTES,
        STANDARD_MTU_IPV4_UDP_PAYLOAD_BYTES
    );
    assert_eq!(
        config().maximum_udp_payload_bytes(),
        STANDARD_MTU_IPV4_UDP_PAYLOAD_BYTES
    );
    assert!(matches!(
        build_udp_frame(
            &config(),
            &[0; STANDARD_MTU_IPV4_UDP_PAYLOAD_BYTES + 1],
            IpPacketOptions::Ipv4 { identification: 1 },
        ),
        Err(EthernetError::PayloadTooLarge(size))
            if size == STANDARD_MTU_IPV4_UDP_PAYLOAD_BYTES + 1
    ));

    let mut oversized = inbound_frame(&payload);
    oversized.push(0);
    oversized[16..18].copy_from_slice(&1501_u16.to_be_bytes());
    oversized[38..40].copy_from_slice(&1481_u16.to_be_bytes());
    set_ipv4_checksum(&mut oversized);
    assert_eq!(
        parse_udp_frame(&oversized, &config()),
        FrameDisposition::Invalid(EthernetError::PayloadTooLarge(
            STANDARD_MTU_IPV4_UDP_PAYLOAD_BYTES + 1
        ))
    );
}

#[test]
fn ipv6_udp_frame_round_trip_is_strict_and_checksummed() {
    let config = ipv6_config();
    assert_eq!(
        config.maximum_udp_payload_bytes(),
        STANDARD_MTU_IPV6_UDP_PAYLOAD_BYTES
    );
    let reverse = RawEthernetConfig::new(
        RAW_ETHERNET_CONFIG_SCHEMA_VERSION,
        "eth0",
        config.board(),
        config.host(),
        8,
    )
    .expect("reverse config");
    let payload = b"ipv6 telemetry";
    let frame = build_udp_frame(&reverse, payload, IpPacketOptions::Ipv6).expect("IPv6 frame");
    assert_eq!(&frame[12..14], &[0x86, 0xdd]);
    assert_eq!(frame[14] >> 4, 6);
    assert_eq!(frame[20], 17);
    assert_eq!(frame[21], 64);
    assert_eq!(&frame[60..62], &[0xbc, 0x89]);
    assert_eq!(
        parse_udp_frame(&frame, &config),
        FrameDisposition::Matched {
            payload: payload.to_vec(),
            sender: Endpoint::new(BOARD_IPV6, BOARD_PORT).expect("board endpoint"),
        }
    );

    let mut missing_checksum = frame.clone();
    missing_checksum[60..62].fill(0);
    assert_eq!(
        parse_udp_frame(&missing_checksum, &config),
        FrameDisposition::Invalid(EthernetError::MissingIpv6UdpChecksum)
    );

    let mut bad_checksum = frame.clone();
    bad_checksum[60] ^= 1;
    assert_eq!(
        parse_udp_frame(&bad_checksum, &config),
        FrameDisposition::Invalid(EthernetError::InvalidIpv6UdpChecksum)
    );

    let mut extension = frame.clone();
    extension[20] = 0;
    assert_eq!(
        parse_udp_frame(&extension, &config),
        FrameDisposition::Invalid(EthernetError::UnsupportedIpv6ExtensionHeader(0))
    );

    assert!(matches!(
        build_udp_frame(
            &config,
            &[0; STANDARD_MTU_IPV6_UDP_PAYLOAD_BYTES + 1],
            IpPacketOptions::Ipv6,
        ),
        Err(EthernetError::PayloadTooLarge(size))
            if size == STANDARD_MTU_IPV6_UDP_PAYLOAD_BYTES + 1
    ));
    assert_eq!(
        build_udp_frame(
            &config,
            payload,
            IpPacketOptions::Ipv4 { identification: 1 },
        ),
        Err(EthernetError::PacketOptionsFamilyMismatch)
    );
}

#[test]
fn ipv4_and_udp_lengths_must_agree() {
    let frame = inbound_frame(&[1, 2, 3]);

    let mut short_ip = frame.clone();
    short_ip[16..18].copy_from_slice(&27_u16.to_be_bytes());
    assert_eq!(
        parse_udp_frame(&short_ip, &config()),
        FrameDisposition::Invalid(EthernetError::InvalidIpv4Header)
    );

    let mut mismatched_ip = frame.clone();
    mismatched_ip[16..18].copy_from_slice(&30_u16.to_be_bytes());
    set_ipv4_checksum(&mut mismatched_ip);
    assert_eq!(
        parse_udp_frame(&mismatched_ip, &config()),
        FrameDisposition::Invalid(EthernetError::InvalidUdpLength)
    );

    let mut mismatched_udp = frame;
    mismatched_udp[38..40].copy_from_slice(&10_u16.to_be_bytes());
    assert_eq!(
        parse_udp_frame(&mismatched_udp, &config()),
        FrameDisposition::Invalid(EthernetError::InvalidUdpLength)
    );
}

#[test]
fn ipv4_fragment_bits_are_rejected_except_dont_fragment() {
    let valid_df = inbound_frame(&[1, 2, 3]);
    assert!(matches!(
        parse_udp_frame(&valid_df, &config()),
        FrameDisposition::Matched { .. }
    ));

    for flags_and_offset in [0x8000_u16, 0x2000, 0x0001] {
        let mut fragmented = valid_df.clone();
        fragmented[20..22].copy_from_slice(&flags_and_offset.to_be_bytes());
        set_ipv4_checksum(&mut fragmented);
        assert_eq!(
            parse_udp_frame(&fragmented, &config()),
            FrameDisposition::Invalid(EthernetError::FragmentedIpv4)
        );
    }
}

#[test]
fn valid_nonzero_udp_checksum_is_accepted() {
    let payload = [1, 2, 3, 4, 5];
    let mut frame = inbound_frame(&payload);
    set_udp_checksum(&mut frame);
    assert_ne!(&frame[40..42], &[0, 0]);
    assert_eq!(
        parse_udp_frame(&frame, &config()),
        FrameDisposition::Matched {
            payload: payload.to_vec(),
            sender: Endpoint::new(BOARD_IP, BOARD_PORT).expect("board endpoint"),
        }
    );
}

#[test]
fn checksum_policy_is_explicit_and_detects_payload_corruption() {
    use ccsds_ethernet_client::Ipv4ChecksumPolicy;
    let config = config();
    let reverse = RawEthernetConfig::new(2, "eth0", config.board(), config.host(), 8)
        .unwrap()
        .with_ipv4_checksum_policy(Ipv4ChecksumPolicy::Generate);
    let required = config
        .clone()
        .with_ipv4_checksum_policy(Ipv4ChecksumPolicy::Require);
    for size in [0, 1, 2, 9, 1472] {
        let payload = vec![0x51; size];
        let mut frame = build_udp_frame(
            &reverse,
            &payload,
            IpPacketOptions::Ipv4 { identification: 0 },
        )
        .unwrap();
        assert_ne!(&frame[40..42], &[0, 0]);
        assert!(matches!(
            parse_udp_frame(&frame, &required),
            FrameDisposition::Matched { .. }
        ));
        if size > 0 {
            frame[42] ^= 1;
            assert_eq!(
                parse_udp_frame(&frame, &required),
                FrameDisposition::Invalid(EthernetError::InvalidUdpChecksum)
            );
        }
    }
    let frame = inbound_frame(b"legacy");
    assert!(matches!(
        parse_udp_frame(&frame, &config),
        FrameDisposition::Matched { .. }
    ));
    assert_eq!(
        parse_udp_frame(&frame, &required),
        FrameDisposition::Invalid(EthernetError::MissingIpv4UdpChecksum)
    );
}

#[test]
fn arbitrary_input_and_truncation_never_panic() {
    let configs = [config(), ipv6_config()];
    let mut state = 0x12345678_u32;
    for config in &configs {
        for length in 0..2049 {
            let bytes: Vec<_> = (0..length)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    state as u8
                })
                .collect();
            let _ = parse_udp_frame(&bytes, config);
        }
    }
    let valid = inbound_frame(&[0x55; 100]);
    for length in 0..valid.len() {
        assert!(!matches!(
            parse_udp_frame(&valid[..length], &config()),
            FrameDisposition::Matched { .. }
        ));
    }
}

#[test]
fn bounded_packet_ring_reports_overflow_and_maximum_depth() {
    let mut ring = BoundedPacketRing::new(2).expect("ring");
    assert!(ring.push(10));
    assert!(ring.push(20));
    assert!(!ring.push(30));
    assert_eq!(ring.len(), 2);
    assert_eq!(ring.maximum_depth(), 2);
    assert_eq!(ring.dropped(), 1);
    assert_eq!(ring.pop(), Some(10));
    assert_eq!(ring.pop(), Some(20));
    assert!(ring.is_empty());
    assert!(BoundedPacketRing::<u8>::new(0).is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn interface_snapshot_validation_is_hardware_free() {
    use ccsds_ethernet_client::InterfaceSnapshot;

    let selected = config();
    let valid = InterfaceSnapshot {
        name: "eth0".to_owned(),
        index: 2,
        mac: selected.host().mac(),
        up: true,
        running: true,
        loopback: false,
    };
    assert!(valid.validate_for(&selected).is_ok());

    let mut wrong = valid;
    wrong.loopback = true;
    assert!(wrong.validate_for(&selected).is_err());
}
