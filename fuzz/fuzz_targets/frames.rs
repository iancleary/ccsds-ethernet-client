#![no_main]
use ccsds_ethernet_client::{
    Endpoint, FrameDisposition, IpPacketOptions, Ipv4ChecksumPolicy, MacAddress, RawEthernetConfig,
    RawEthernetEndpoint, build_udp_frame, parse_udp_frame,
};
use libfuzzer_sys::fuzz_target;
use std::net::IpAddr;

fuzz_target!(|data: &[u8]| {
    for (host, peer) in [("192.0.2.1", "192.0.2.2"), ("2001:db8::1", "2001:db8::2")] {
        let host = RawEthernetEndpoint::new(
            MacAddress::new([2, 0, 0, 0, 0, 1]),
            Endpoint::new(host.parse::<IpAddr>().unwrap(), 40001).unwrap(),
        )
        .unwrap();
        let peer = RawEthernetEndpoint::new(
            MacAddress::new([2, 0, 0, 0, 0, 2]),
            Endpoint::new(peer.parse::<IpAddr>().unwrap(), 40002).unwrap(),
        )
        .unwrap();
        let config = RawEthernetConfig::new(2, "eth0", host, peer, 1)
            .unwrap()
            .with_ipv4_checksum_policy(Ipv4ChecksumPolicy::Require);
        let _ = parse_udp_frame(data, &config);
        // Structured seeds reach length, endpoint, fragmentation, and checksum
        // checks even when raw arbitrary input fails at the Ethernet header.
        let outgoing = RawEthernetConfig::new(2, "eth0", peer, host, 1)
            .unwrap()
            .with_ipv4_checksum_policy(Ipv4ChecksumPolicy::Require);
        let payload = &data[..data.len().min(1400)];
        let options = if host.network().as_ipv4().is_some() {
            IpPacketOptions::Ipv4 { identification: 0 }
        } else {
            IpPacketOptions::Ipv6
        };
        let mut frame = build_udp_frame(&outgoing, payload, options).unwrap();
        assert!(
            matches!(parse_udp_frame(&frame, &config), FrameDisposition::Matched { payload: decoded, .. } if decoded == payload)
        );
        if let Some(&offset) = data.first() {
            let index = usize::from(offset) % frame.len();
            frame[index] ^= data.get(1).copied().unwrap_or(1);
            let _ = parse_udp_frame(&frame, &config);
            let _ = parse_udp_frame(&frame[..index], &config);
        }
    }
});
