//! Typed, in-process CCSDS Ethernet client transport.
//!
//! This crate owns Ethernet/IPv4/UDP transport and session lifecycle only.
//! Mission codecs own packet schemas, correlation, acknowledgement meaning,
//! retry policy, and actuation policy.

mod endpoint;
pub mod ethernet;
#[cfg(target_os = "linux")]
pub mod linux;
mod session;
pub mod transport;

pub use endpoint::{Endpoint, EndpointError};
pub use ethernet::{
    EthernetError, FrameDisposition, MacAddress, RAW_ETHERNET_CONFIG_SCHEMA_VERSION,
    RawEthernetConfig, RawEthernetEndpoint, STANDARD_MTU_UDP_PAYLOAD_BYTES, build_udp_frame,
    parse_udp_frame, validate_interface_name,
};
#[cfg(target_os = "linux")]
pub use linux::{InterfaceSnapshot, LinuxRawEthernetTransport, inspect_interface};
pub use session::{
    Codec, DecodeResult, DecodedMessage, EncodedCommand, ExchangeError, ReceiveError, Session,
    SessionStatistics,
};
pub use transport::{
    BoundedPacketRing, MemoryTransport, MemoryTransportEvent, ReceivedFrame, Transport,
    TransportError, TransportStatistics,
};
