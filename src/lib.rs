//! Typed, in-process CCSDS Ethernet client transport.
//!
//! This crate owns Ethernet/IP/UDP transport and session lifecycle only.
//! Mission codecs own packet schemas, correlation, acknowledgement meaning,
//! retry policy, and actuation policy.

mod endpoint;
pub mod ethernet;
#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(feature = "python")]
mod python;
mod session;
pub mod transport;

#[rustfmt::skip]
pub use endpoint::{
    Endpoint,
    EndpointError,
};
#[rustfmt::skip]
#[allow(deprecated)]
pub use ethernet::{
    EthernetError,
    FrameDisposition,
    IpPacketOptions,
    Ipv4ChecksumPolicy,
    MacAddress,
    RAW_ETHERNET_CONFIG_SCHEMA_VERSION,
    RawEthernetConfig,
    RawEthernetEndpoint,
    STANDARD_MTU_IPV4_UDP_PAYLOAD_BYTES,
    STANDARD_MTU_IPV6_UDP_PAYLOAD_BYTES,
    STANDARD_MTU_UDP_PAYLOAD_BYTES,
    build_udp_frame,
    parse_udp_frame,
    validate_interface_name,
};
#[cfg(target_os = "linux")]
#[rustfmt::skip]
pub use linux::{
    InterfaceSnapshot,
    LinuxRawEthernetTransport,
    inspect_interface,
};
#[rustfmt::skip]
pub use session::{
    Codec,
    DecodeResult,
    DecodedMessage,
    EncodedCommand,
    ExchangeError,
    ReceiveError,
    Session,
    SessionDiagnostics,
    SessionStatistics,
};
#[rustfmt::skip]
pub use transport::{
    BoundedPacketRing,
    FrameClassificationStatistics,
    MemoryTransport,
    MemoryTransportEvent,
    ReceivedFrame,
    Transport,
    TransportError,
    TransportStatistics,
};
