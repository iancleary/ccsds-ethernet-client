use std::error::Error;
use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;

use crate::Endpoint;

pub const RAW_ETHERNET_CONFIG_SCHEMA_VERSION: u16 = 2;

pub const ETHERNET_HEADER_BYTES: usize = 14;
pub const IPV4_HEADER_BYTES: usize = 20;
pub const IPV6_HEADER_BYTES: usize = 40;
pub const UDP_HEADER_BYTES: usize = 8;
pub const MINIMUM_ETHERNET_FRAME_BYTES: usize = 60;
pub const STANDARD_MTU_IPV4_UDP_PAYLOAD_BYTES: usize = 1_500 - IPV4_HEADER_BYTES - UDP_HEADER_BYTES;
pub const STANDARD_MTU_IPV6_UDP_PAYLOAD_BYTES: usize = 1_500 - IPV6_HEADER_BYTES - UDP_HEADER_BYTES;
#[deprecated(
    since = "0.2.0",
    note = "use STANDARD_MTU_IPV4_UDP_PAYLOAD_BYTES or RawEthernetConfig::maximum_udp_payload_bytes"
)]
pub const STANDARD_MTU_UDP_PAYLOAD_BYTES: usize = STANDARD_MTU_IPV4_UDP_PAYLOAD_BYTES;
const ETHERTYPE_IPV4: u16 = 0x0800;
const ETHERTYPE_IPV6: u16 = 0x86dd;
const IP_PROTOCOL_UDP: u8 = 17;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct MacAddress([u8; 6]);

impl MacAddress {
    pub const fn new(bytes: [u8; 6]) -> Self {
        Self(bytes)
    }

    pub const fn octets(self) -> [u8; 6] {
        self.0
    }

    pub const fn is_unicast(self) -> bool {
        self.0[0] & 1 == 0
    }

    pub const fn is_zero(self) -> bool {
        self.0[0] == 0
            && self.0[1] == 0
            && self.0[2] == 0
            && self.0[3] == 0
            && self.0[4] == 0
            && self.0[5] == 0
    }
}

impl fmt::Display for MacAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            self.0[0], self.0[1], self.0[2], self.0[3], self.0[4], self.0[5]
        )
    }
}

impl FromStr for MacAddress {
    type Err = EthernetError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let parts = value.split(':').collect::<Vec<_>>();
        if parts.len() != 6 || parts.iter().any(|part| part.len() != 2) {
            return Err(EthernetError::InvalidMac(value.to_owned()));
        }
        let mut bytes = [0_u8; 6];
        for (index, part) in parts.into_iter().enumerate() {
            bytes[index] = u8::from_str_radix(part, 16)
                .map_err(|_| EthernetError::InvalidMac(value.to_owned()))?;
        }
        let address = Self(bytes);
        if address.is_zero() || !address.is_unicast() {
            return Err(EthernetError::InvalidMac(value.to_owned()));
        }
        Ok(address)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RawEthernetEndpoint {
    mac: MacAddress,
    network: Endpoint,
}

impl RawEthernetEndpoint {
    pub fn new(mac: MacAddress, network: Endpoint) -> Result<Self, EthernetError> {
        if mac.is_zero() || !mac.is_unicast() {
            return Err(EthernetError::InvalidMac(mac.to_string()));
        }
        Ok(Self { mac, network })
    }

    pub const fn mac(self) -> MacAddress {
        self.mac
    }

    pub const fn network(self) -> Endpoint {
        self.network
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawEthernetConfig {
    ipv4_checksum_policy: Ipv4ChecksumPolicy,
    schema_version: u16,
    interface_name: String,
    host: RawEthernetEndpoint,
    board: RawEthernetEndpoint,
    ring_capacity: usize,
}

/// IPv4 UDP checksum interoperability policy. IPv6 always requires checksums.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Ipv4ChecksumPolicy {
    /// Preserve the original wire behavior: transmit zero, accept zero.
    #[default]
    Legacy,
    /// Generate checksums, but accept legacy zero-checksum peers.
    Generate,
    /// Generate checksums and reject zero-checksum packets.
    Require,
}

impl RawEthernetConfig {
    pub fn new(
        schema_version: u16,
        interface_name: impl Into<String>,
        host: RawEthernetEndpoint,
        board: RawEthernetEndpoint,
        ring_capacity: usize,
    ) -> Result<Self, EthernetError> {
        let config = Self {
            ipv4_checksum_policy: Ipv4ChecksumPolicy::Legacy,
            schema_version,
            interface_name: interface_name.into(),
            host,
            board,
            ring_capacity,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), EthernetError> {
        if self.schema_version != RAW_ETHERNET_CONFIG_SCHEMA_VERSION {
            return Err(EthernetError::UnsupportedSchemaVersion(self.schema_version));
        }
        validate_interface_name(&self.interface_name)?;
        if self.host.mac == self.board.mac {
            return Err(EthernetError::InvalidConfig(
                "host and board MAC addresses must differ".to_owned(),
            ));
        }
        if self.host.network == self.board.network {
            return Err(EthernetError::InvalidConfig(
                "host and board endpoints must differ".to_owned(),
            ));
        }
        match (self.host.network.ip(), self.board.network.ip()) {
            (IpAddr::V4(host), IpAddr::V4(board)) if host == board => {
                return Err(EthernetError::InvalidConfig(
                    "host and board IPv4 addresses must differ".to_owned(),
                ));
            }
            (IpAddr::V6(host), IpAddr::V6(board)) if host == board => {
                return Err(EthernetError::InvalidConfig(
                    "host and board IPv6 addresses must differ".to_owned(),
                ));
            }
            (IpAddr::V4(_), IpAddr::V4(_)) | (IpAddr::V6(_), IpAddr::V6(_)) => {}
            (host, board) => return Err(EthernetError::MixedIpFamilies { host, board }),
        }
        if self.ring_capacity == 0 {
            return Err(EthernetError::InvalidConfig(
                "packet ring capacity must be at least 1".to_owned(),
            ));
        }
        Ok(())
    }

    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    pub fn with_ipv4_checksum_policy(mut self, policy: Ipv4ChecksumPolicy) -> Self {
        self.ipv4_checksum_policy = policy;
        self
    }

    pub const fn ipv4_checksum_policy(&self) -> Ipv4ChecksumPolicy {
        self.ipv4_checksum_policy
    }

    pub fn interface_name(&self) -> &str {
        &self.interface_name
    }

    pub const fn host(&self) -> RawEthernetEndpoint {
        self.host
    }

    pub const fn board(&self) -> RawEthernetEndpoint {
        self.board
    }

    pub const fn ring_capacity(&self) -> usize {
        self.ring_capacity
    }

    pub const fn maximum_udp_payload_bytes(&self) -> usize {
        match self.host.network.ip() {
            IpAddr::V4(_) => STANDARD_MTU_IPV4_UDP_PAYLOAD_BYTES,
            IpAddr::V6(_) => STANDARD_MTU_IPV6_UDP_PAYLOAD_BYTES,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FrameDisposition {
    Matched { payload: Vec<u8>, sender: Endpoint },
    Foreign,
    Invalid(EthernetError),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EthernetError {
    InvalidMac(String),
    InvalidInterfaceName(String),
    UnsupportedSchemaVersion(u16),
    InvalidConfig(String),
    PayloadTooLarge(usize),
    FrameTooShort(usize),
    UnsupportedEtherType(u16),
    InvalidIpv4Header,
    FragmentedIpv4,
    InvalidIpv4Checksum,
    InvalidIpv6Header,
    UnsupportedIpv6ExtensionHeader(u8),
    InvalidIpv6PayloadLength,
    MissingIpv6UdpChecksum,
    InvalidIpv6UdpChecksum,
    InvalidUdpLength,
    InvalidUdpChecksum,
    MissingIpv4UdpChecksum,
    MixedIpFamilies { host: IpAddr, board: IpAddr },
    PacketOptionsFamilyMismatch,
}

impl fmt::Display for EthernetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMac(value) => write!(formatter, "invalid unicast MAC address {value:?}"),
            Self::InvalidInterfaceName(value) => {
                write!(formatter, "invalid Linux interface name {value:?}")
            }
            Self::UnsupportedSchemaVersion(version) => write!(
                formatter,
                "unsupported raw Ethernet config schema version {version}; expected {RAW_ETHERNET_CONFIG_SCHEMA_VERSION}"
            ),
            Self::InvalidConfig(message) => formatter.write_str(message),
            Self::PayloadTooLarge(size) => write!(formatter, "UDP payload is too large: {size}"),
            Self::FrameTooShort(size) => write!(formatter, "Ethernet frame is too short: {size}"),
            Self::UnsupportedEtherType(value) => {
                write!(formatter, "unsupported EtherType 0x{value:04x}")
            }
            Self::InvalidIpv4Header => formatter.write_str("invalid IPv4 header or length"),
            Self::FragmentedIpv4 => formatter.write_str("fragmented IPv4 is not supported"),
            Self::InvalidIpv4Checksum => formatter.write_str("invalid IPv4 header checksum"),
            Self::InvalidIpv6Header => formatter.write_str("invalid IPv6 header"),
            Self::UnsupportedIpv6ExtensionHeader(value) => {
                write!(
                    formatter,
                    "unsupported IPv6 extension or next header {value}"
                )
            }
            Self::InvalidIpv6PayloadLength => formatter.write_str("invalid IPv6 payload length"),
            Self::MissingIpv6UdpChecksum => formatter.write_str("missing IPv6 UDP checksum"),
            Self::InvalidIpv6UdpChecksum => formatter.write_str("invalid IPv6 UDP checksum"),
            Self::InvalidUdpLength => formatter.write_str("invalid UDP length"),
            Self::InvalidUdpChecksum => formatter.write_str("invalid UDP checksum"),
            Self::MissingIpv4UdpChecksum => {
                formatter.write_str("missing required IPv4 UDP checksum")
            }
            Self::MixedIpFamilies { host, board } => write!(
                formatter,
                "host and board IP addresses must use the same family, got {host} and {board}"
            ),
            Self::PacketOptionsFamilyMismatch => {
                formatter.write_str("packet options do not match configured IP family")
            }
        }
    }
}

impl Error for EthernetError {}

pub fn validate_interface_name(value: &str) -> Result<(), EthernetError> {
    let valid = !value.is_empty()
        && value.len() < 16
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'));
    if valid {
        Ok(())
    } else {
        Err(EthernetError::InvalidInterfaceName(value.to_owned()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IpPacketOptions {
    Ipv4 { identification: u16 },
    Ipv6,
}

pub fn build_udp_frame(
    config: &RawEthernetConfig,
    payload: &[u8],
    options: IpPacketOptions,
) -> Result<Vec<u8>, EthernetError> {
    config.validate()?;
    match (
        config.host().network().ip(),
        config.board().network().ip(),
        options,
    ) {
        (IpAddr::V4(_), IpAddr::V4(_), IpPacketOptions::Ipv4 { identification }) => {
            build_ipv4_udp_frame(config, payload, identification)
        }
        (IpAddr::V6(_), IpAddr::V6(_), IpPacketOptions::Ipv6) => {
            build_ipv6_udp_frame(config, payload)
        }
        (host, board, _) if host.is_ipv4() != board.is_ipv4() => {
            Err(EthernetError::MixedIpFamilies { host, board })
        }
        (_, _, _) => Err(EthernetError::PacketOptionsFamilyMismatch),
    }
}

fn build_ipv4_udp_frame(
    config: &RawEthernetConfig,
    payload: &[u8],
    ipv4_identification: u16,
) -> Result<Vec<u8>, EthernetError> {
    if payload.len() > STANDARD_MTU_IPV4_UDP_PAYLOAD_BYTES {
        return Err(EthernetError::PayloadTooLarge(payload.len()));
    }
    let udp_length = UDP_HEADER_BYTES
        .checked_add(payload.len())
        .and_then(|length| u16::try_from(length).ok())
        .ok_or(EthernetError::PayloadTooLarge(payload.len()))?;
    let ip_length = u16::try_from(IPV4_HEADER_BYTES + usize::from(udp_length))
        .map_err(|_| EthernetError::PayloadTooLarge(payload.len()))?;
    let frame_length = ETHERNET_HEADER_BYTES + usize::from(ip_length);
    let mut frame = vec![0_u8; frame_length.max(MINIMUM_ETHERNET_FRAME_BYTES)];

    frame[0..6].copy_from_slice(&config.board.mac.octets());
    frame[6..12].copy_from_slice(&config.host.mac.octets());
    frame[12..14].copy_from_slice(&ETHERTYPE_IPV4.to_be_bytes());

    let ip = ETHERNET_HEADER_BYTES;
    frame[ip] = 0x45;
    frame[ip + 2..ip + 4].copy_from_slice(&ip_length.to_be_bytes());
    frame[ip + 4..ip + 6].copy_from_slice(&ipv4_identification.to_be_bytes());
    frame[ip + 6..ip + 8].copy_from_slice(&0x4000_u16.to_be_bytes());
    frame[ip + 8] = 64;
    frame[ip + 9] = IP_PROTOCOL_UDP;
    let host = config
        .host
        .network
        .as_ipv4()
        .ok_or(EthernetError::PacketOptionsFamilyMismatch)?;
    let board = config
        .board
        .network
        .as_ipv4()
        .ok_or(EthernetError::PacketOptionsFamilyMismatch)?;
    frame[ip + 12..ip + 16].copy_from_slice(&host.octets());
    frame[ip + 16..ip + 20].copy_from_slice(&board.octets());
    let checksum = internet_checksum(&frame[ip..ip + IPV4_HEADER_BYTES]);
    frame[ip + 10..ip + 12].copy_from_slice(&checksum.to_be_bytes());

    let udp = ip + IPV4_HEADER_BYTES;
    frame[udp..udp + 2].copy_from_slice(&config.host.network.udp_port().to_be_bytes());
    frame[udp + 2..udp + 4].copy_from_slice(&config.board.network.udp_port().to_be_bytes());
    frame[udp + 4..udp + 6].copy_from_slice(&udp_length.to_be_bytes());
    // IPv4 permits a zero UDP checksum; retain it only in legacy transmit mode.
    frame[udp + UDP_HEADER_BYTES..udp + usize::from(udp_length)].copy_from_slice(payload);
    if config.ipv4_checksum_policy != Ipv4ChecksumPolicy::Legacy {
        let mut pseudo = Vec::with_capacity(12 + usize::from(udp_length));
        pseudo.extend_from_slice(&host.octets());
        pseudo.extend_from_slice(&board.octets());
        pseudo.extend_from_slice(&[0, IP_PROTOCOL_UDP]);
        pseudo.extend_from_slice(&udp_length.to_be_bytes());
        pseudo.extend_from_slice(&frame[udp..udp + usize::from(udp_length)]);
        let checksum = internet_checksum(&pseudo);
        frame[udp + 6..udp + 8]
            .copy_from_slice(&if checksum == 0 { 0xffff } else { checksum }.to_be_bytes());
    }
    Ok(frame)
}

fn build_ipv6_udp_frame(
    config: &RawEthernetConfig,
    payload: &[u8],
) -> Result<Vec<u8>, EthernetError> {
    if payload.len() > STANDARD_MTU_IPV6_UDP_PAYLOAD_BYTES {
        return Err(EthernetError::PayloadTooLarge(payload.len()));
    }
    let udp_length = UDP_HEADER_BYTES
        .checked_add(payload.len())
        .and_then(|length| u16::try_from(length).ok())
        .ok_or(EthernetError::PayloadTooLarge(payload.len()))?;
    let frame_length = ETHERNET_HEADER_BYTES + IPV6_HEADER_BYTES + usize::from(udp_length);
    let mut frame = vec![0_u8; frame_length.max(MINIMUM_ETHERNET_FRAME_BYTES)];

    frame[0..6].copy_from_slice(&config.board.mac.octets());
    frame[6..12].copy_from_slice(&config.host.mac.octets());
    frame[12..14].copy_from_slice(&ETHERTYPE_IPV6.to_be_bytes());

    let ip = ETHERNET_HEADER_BYTES;
    frame[ip] = 0x60;
    frame[ip + 4..ip + 6].copy_from_slice(&udp_length.to_be_bytes());
    frame[ip + 6] = IP_PROTOCOL_UDP;
    frame[ip + 7] = 64;
    let host = config
        .host
        .network
        .as_ipv6()
        .ok_or(EthernetError::PacketOptionsFamilyMismatch)?;
    let board = config
        .board
        .network
        .as_ipv6()
        .ok_or(EthernetError::PacketOptionsFamilyMismatch)?;
    frame[ip + 8..ip + 24].copy_from_slice(&host.octets());
    frame[ip + 24..ip + 40].copy_from_slice(&board.octets());

    let udp = ip + IPV6_HEADER_BYTES;
    frame[udp..udp + 2].copy_from_slice(&config.host.network.udp_port().to_be_bytes());
    frame[udp + 2..udp + 4].copy_from_slice(&config.board.network.udp_port().to_be_bytes());
    frame[udp + 4..udp + 6].copy_from_slice(&udp_length.to_be_bytes());
    frame[udp + UDP_HEADER_BYTES..udp + usize::from(udp_length)].copy_from_slice(payload);
    let checksum = udp_checksum_ipv6(
        &host.octets(),
        &board.octets(),
        &frame[udp..udp + usize::from(udp_length)],
    );
    frame[udp + 6..udp + 8].copy_from_slice(&checksum.to_be_bytes());
    Ok(frame)
}

pub fn parse_udp_frame(frame: &[u8], config: &RawEthernetConfig) -> FrameDisposition {
    if frame.len() < ETHERNET_HEADER_BYTES {
        return FrameDisposition::Invalid(EthernetError::FrameTooShort(frame.len()));
    }
    let ether_type = u16::from_be_bytes([frame[12], frame[13]]);
    match ether_type {
        ETHERTYPE_IPV4 if config.host().network().as_ipv4().is_some() => {
            parse_ipv4_udp_frame(frame, config)
        }
        ETHERTYPE_IPV6 if config.host().network().as_ipv6().is_some() => {
            parse_ipv6_udp_frame(frame, config)
        }
        _ => FrameDisposition::Invalid(EthernetError::UnsupportedEtherType(ether_type)),
    }
}

fn parse_ipv4_udp_frame(frame: &[u8], config: &RawEthernetConfig) -> FrameDisposition {
    if frame.len() < ETHERNET_HEADER_BYTES + IPV4_HEADER_BYTES + UDP_HEADER_BYTES {
        return FrameDisposition::Invalid(EthernetError::FrameTooShort(frame.len()));
    }
    let ip = ETHERNET_HEADER_BYTES;
    let ihl = usize::from(frame[ip] & 0x0f) * 4;
    if frame[ip] >> 4 != 4 || ihl != IPV4_HEADER_BYTES || frame[ip + 9] != IP_PROTOCOL_UDP {
        return FrameDisposition::Invalid(EthernetError::InvalidIpv4Header);
    }
    let total_length = usize::from(u16::from_be_bytes([frame[ip + 2], frame[ip + 3]]));
    if total_length < IPV4_HEADER_BYTES + UDP_HEADER_BYTES
        || ETHERNET_HEADER_BYTES + total_length > frame.len()
    {
        return FrameDisposition::Invalid(EthernetError::InvalidIpv4Header);
    }
    if total_length > IPV4_HEADER_BYTES + UDP_HEADER_BYTES + STANDARD_MTU_IPV4_UDP_PAYLOAD_BYTES {
        return FrameDisposition::Invalid(EthernetError::PayloadTooLarge(
            total_length - IPV4_HEADER_BYTES - UDP_HEADER_BYTES,
        ));
    }
    let fragment = u16::from_be_bytes([frame[ip + 6], frame[ip + 7]]);
    // Reject the reserved flag, More Fragments, and any fragment offset. DF is valid.
    if fragment & 0xbfff != 0 {
        return FrameDisposition::Invalid(EthernetError::FragmentedIpv4);
    }
    if internet_checksum(&frame[ip..ip + ihl]) != 0 {
        return FrameDisposition::Invalid(EthernetError::InvalidIpv4Checksum);
    }

    let udp = ip + ihl;
    let udp_length = usize::from(u16::from_be_bytes([frame[udp + 4], frame[udp + 5]]));
    if udp_length < UDP_HEADER_BYTES || ihl + udp_length != total_length {
        return FrameDisposition::Invalid(EthernetError::InvalidUdpLength);
    }
    let destination_mac = &frame[0..6];
    let source_mac = &frame[6..12];
    let source_ip = &frame[ip + 12..ip + 16];
    let destination_ip = &frame[ip + 16..ip + 20];
    let source_port = u16::from_be_bytes([frame[udp], frame[udp + 1]]);
    let destination_port = u16::from_be_bytes([frame[udp + 2], frame[udp + 3]]);
    let expected_source_ip = config
        .board
        .network
        .as_ipv4()
        .expect("IPv4 config")
        .octets();
    let expected_destination_ip = config.host.network.as_ipv4().expect("IPv4 config").octets();
    if destination_mac != config.host.mac.octets()
        || source_mac != config.board.mac.octets()
        || source_ip != expected_source_ip
        || destination_ip != expected_destination_ip
        || source_port != config.board.network.udp_port()
        || destination_port != config.host.network.udp_port()
    {
        return FrameDisposition::Foreign;
    }

    let udp_checksum = u16::from_be_bytes([frame[udp + 6], frame[udp + 7]]);
    if udp_checksum == 0 && config.ipv4_checksum_policy == Ipv4ChecksumPolicy::Require {
        return FrameDisposition::Invalid(EthernetError::MissingIpv4UdpChecksum);
    }
    if udp_checksum != 0
        && !udp_checksum_valid(
            &frame[ip + 12..ip + 16],
            &frame[ip + 16..ip + 20],
            &frame[udp..udp + udp_length],
        )
    {
        return FrameDisposition::Invalid(EthernetError::InvalidUdpChecksum);
    }

    FrameDisposition::Matched {
        payload: frame[udp + UDP_HEADER_BYTES..udp + udp_length].to_vec(),
        sender: config.board.network,
    }
}

fn parse_ipv6_udp_frame(frame: &[u8], config: &RawEthernetConfig) -> FrameDisposition {
    if frame.len() < ETHERNET_HEADER_BYTES + IPV6_HEADER_BYTES + UDP_HEADER_BYTES {
        return FrameDisposition::Invalid(EthernetError::FrameTooShort(frame.len()));
    }

    let ip = ETHERNET_HEADER_BYTES;
    if frame[ip] >> 4 != 6 {
        return FrameDisposition::Invalid(EthernetError::InvalidIpv6Header);
    }
    let payload_length = usize::from(u16::from_be_bytes([frame[ip + 4], frame[ip + 5]]));
    if payload_length < UDP_HEADER_BYTES
        || ETHERNET_HEADER_BYTES + IPV6_HEADER_BYTES + payload_length > frame.len()
    {
        return FrameDisposition::Invalid(EthernetError::InvalidIpv6PayloadLength);
    }
    if frame[ip + 6] != IP_PROTOCOL_UDP {
        return FrameDisposition::Invalid(EthernetError::UnsupportedIpv6ExtensionHeader(
            frame[ip + 6],
        ));
    }
    if payload_length > UDP_HEADER_BYTES + STANDARD_MTU_IPV6_UDP_PAYLOAD_BYTES {
        return FrameDisposition::Invalid(EthernetError::InvalidIpv6PayloadLength);
    }

    let udp = ip + IPV6_HEADER_BYTES;
    let udp_length = usize::from(u16::from_be_bytes([frame[udp + 4], frame[udp + 5]]));
    if udp_length < UDP_HEADER_BYTES || udp_length != payload_length {
        return FrameDisposition::Invalid(EthernetError::InvalidUdpLength);
    }
    let destination_mac = &frame[0..6];
    let source_mac = &frame[6..12];
    let source_ip = &frame[ip + 8..ip + 24];
    let destination_ip = &frame[ip + 24..ip + 40];
    let source_port = u16::from_be_bytes([frame[udp], frame[udp + 1]]);
    let destination_port = u16::from_be_bytes([frame[udp + 2], frame[udp + 3]]);
    let expected_source_ip = config
        .board
        .network
        .as_ipv6()
        .expect("IPv6 config")
        .octets();
    let expected_destination_ip = config.host.network.as_ipv6().expect("IPv6 config").octets();
    if destination_mac != config.host.mac.octets()
        || source_mac != config.board.mac.octets()
        || source_ip != expected_source_ip
        || destination_ip != expected_destination_ip
        || source_port != config.board.network.udp_port()
        || destination_port != config.host.network.udp_port()
    {
        return FrameDisposition::Foreign;
    }

    let udp_checksum = u16::from_be_bytes([frame[udp + 6], frame[udp + 7]]);
    if udp_checksum == 0 {
        return FrameDisposition::Invalid(EthernetError::MissingIpv6UdpChecksum);
    }
    if !udp_checksum_valid_ipv6(
        &frame[ip + 8..ip + 24],
        &frame[ip + 24..ip + 40],
        &frame[udp..udp + udp_length],
    ) {
        return FrameDisposition::Invalid(EthernetError::InvalidIpv6UdpChecksum);
    }

    FrameDisposition::Matched {
        payload: frame[udp + UDP_HEADER_BYTES..udp + udp_length].to_vec(),
        sender: config.board.network,
    }
}

fn internet_checksum(bytes: &[u8]) -> u16 {
    let mut sum = 0_u32;
    for chunk in bytes.chunks(2) {
        let word = if chunk.len() == 2 {
            u16::from_be_bytes([chunk[0], chunk[1]])
        } else {
            u16::from(chunk[0]) << 8
        };
        sum += u32::from(word);
    }
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

fn udp_checksum_valid(source: &[u8], destination: &[u8], udp: &[u8]) -> bool {
    let mut pseudo = Vec::with_capacity(12 + udp.len());
    pseudo.extend_from_slice(source);
    pseudo.extend_from_slice(destination);
    pseudo.push(0);
    pseudo.push(IP_PROTOCOL_UDP);
    pseudo.extend_from_slice(&(udp.len() as u16).to_be_bytes());
    pseudo.extend_from_slice(udp);
    internet_checksum(&pseudo) == 0
}

fn udp_checksum_ipv6(source: &[u8; 16], destination: &[u8; 16], udp: &[u8]) -> u16 {
    let mut pseudo = Vec::with_capacity(40 + udp.len());
    pseudo.extend_from_slice(source);
    pseudo.extend_from_slice(destination);
    pseudo.extend_from_slice(&(udp.len() as u32).to_be_bytes());
    pseudo.extend_from_slice(&[0, 0, 0]);
    pseudo.push(IP_PROTOCOL_UDP);
    pseudo.extend_from_slice(udp);
    let checksum = internet_checksum(&pseudo);
    if checksum == 0 { 0xffff } else { checksum }
}

fn udp_checksum_valid_ipv6(source: &[u8], destination: &[u8], udp: &[u8]) -> bool {
    let mut pseudo = Vec::with_capacity(40 + udp.len());
    pseudo.extend_from_slice(source);
    pseudo.extend_from_slice(destination);
    pseudo.extend_from_slice(&(udp.len() as u32).to_be_bytes());
    pseudo.extend_from_slice(&[0, 0, 0]);
    pseudo.push(IP_PROTOCOL_UDP);
    pseudo.extend_from_slice(udp);
    internet_checksum(&pseudo) == 0
}
