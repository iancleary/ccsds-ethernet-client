//! Linux `AF_PACKET` transport.
//!
//! The unsafe surface is intentionally limited to libc calls and pointer
//! conversions in this module. Packet construction, parsing, correlation,
//! and buffering remain safe Rust.

use std::ffi::CStr;
use std::io;
use std::mem;
use std::os::fd::RawFd;
use std::ptr;
use std::time::Instant;

#[rustfmt::skip]
use crate::ethernet::{
    ETHERNET_HEADER_BYTES,
    EthernetError,
    FrameDisposition,
    IpPacketOptions,
    MacAddress,
    RawEthernetConfig,
    build_udp_frame,
    parse_udp_frame,
};
#[rustfmt::skip]
use crate::transport::{
    BoundedPacketRing,
    FrameClassificationStatistics,
    ReceivedFrame,
    Transport,
    TransportError,
    TransportStatistics,
};

const PACKET_STATISTICS: libc::c_int = 6;
#[cfg(test)]
const MINIMUM_IPV4_UDP_FRAME_BYTES: usize = 42;
const MAXIMUM_FRAME_BYTES: usize = 2048;
const RECEIVE_DRAIN_PACKET_BUDGET: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InterfaceSnapshot {
    pub name: String,
    pub index: i32,
    pub mac: MacAddress,
    pub up: bool,
    pub running: bool,
    pub loopback: bool,
}

impl InterfaceSnapshot {
    pub fn validate_for(&self, config: &RawEthernetConfig) -> Result<(), TransportError> {
        if self.name != config.interface_name() {
            return Err(other(format!(
                "interface name changed: expected {:?}, got {:?}",
                config.interface_name(),
                self.name
            )));
        }
        if self.mac != config.host().mac() {
            return Err(other(format!(
                "interface MAC mismatch on {}: expected {}, got {}",
                self.name,
                config.host().mac(),
                self.mac
            )));
        }
        if self.loopback {
            return Err(other(format!(
                "interface {} is loopback, not a dedicated Ethernet link",
                self.name
            )));
        }
        if !self.up || !self.running {
            return Err(other(format!(
                "interface {} is not both UP and RUNNING",
                self.name
            )));
        }
        Ok(())
    }
}

pub fn inspect_interface(name: &str) -> Result<InterfaceSnapshot, TransportError> {
    crate::ethernet::validate_interface_name(name).map_err(|error| other(error.to_string()))?;
    let mut addresses: *mut libc::ifaddrs = ptr::null_mut();
    // SAFETY: `addresses` is a valid out-pointer. A successful call returns a
    // linked list owned by libc and released exactly once by `IfAddrsGuard`.
    if unsafe { libc::getifaddrs(&mut addresses) } != 0 {
        return Err(last_os_error("getifaddrs"));
    }
    let guard = IfAddrsGuard(addresses);
    let mut current = guard.0;
    while !current.is_null() {
        // SAFETY: every node is part of the live getifaddrs list guarded above.
        let entry = unsafe { &*current };
        if !entry.ifa_name.is_null() && !entry.ifa_addr.is_null() {
            // SAFETY: libc guarantees `ifa_name` is a NUL-terminated string.
            let entry_name = unsafe { CStr::from_ptr(entry.ifa_name) }.to_string_lossy();
            // SAFETY: `ifa_addr` is non-null and its family identifies the
            // concrete sockaddr layout before the cast.
            let family = unsafe { (*entry.ifa_addr).sa_family as libc::c_int };
            if entry_name == name && family == libc::AF_PACKET {
                // SAFETY: AF_PACKET entries use sockaddr_ll.
                let address = unsafe { &*(entry.ifa_addr.cast::<libc::sockaddr_ll>()) };
                let (index, mac) = ethernet_packet_metadata(name, address)?;
                let flags = entry.ifa_flags as libc::c_int;
                return Ok(InterfaceSnapshot {
                    name: entry_name.into_owned(),
                    index,
                    mac,
                    up: flags & libc::IFF_UP != 0,
                    running: flags & libc::IFF_RUNNING != 0,
                    loopback: flags & libc::IFF_LOOPBACK != 0,
                });
            }
        }
        current = entry.ifa_next;
    }
    Err(other(format!("Linux interface {name:?} was not found")))
}

fn ethernet_packet_metadata(
    name: &str,
    address: &libc::sockaddr_ll,
) -> Result<(i32, MacAddress), TransportError> {
    if address.sll_hatype != libc::ARPHRD_ETHER {
        return Err(other(format!("interface {name} is not Ethernet hardware")));
    }
    if address.sll_halen != 6 {
        return Err(other(format!(
            "interface {name} does not have a 6-byte Ethernet address"
        )));
    }
    if address.sll_ifindex <= 0 {
        return Err(other(format!(
            "interface {name} has invalid index {}",
            address.sll_ifindex
        )));
    }
    Ok((
        address.sll_ifindex,
        MacAddress::new(address.sll_addr[..6].try_into().expect("six bytes")),
    ))
}

struct IfAddrsGuard(*mut libc::ifaddrs);

impl Drop for IfAddrsGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: this pointer came from successful getifaddrs and has not
            // been freed elsewhere.
            unsafe { libc::freeifaddrs(self.0) };
        }
    }
}

struct PacketSocket(RawFd);

impl PacketSocket {
    fn close(&mut self) -> io::Result<()> {
        self.close_with(close_descriptor)
    }

    fn close_with(&mut self, close: impl FnOnce(RawFd) -> io::Result<()>) -> io::Result<()> {
        let descriptor = mem::replace(&mut self.0, -1);
        if descriptor < 0 {
            return Ok(());
        }
        close(descriptor)
    }
}

impl Drop for PacketSocket {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

pub struct LinuxRawEthernetTransport {
    socket: PacketSocket,
    config: RawEthernetConfig,
    interface_index: i32,
    ring: BoundedPacketRing<ReceivedFrame>,
    statistics: TransportStatistics,
    receive_ready: bool,
    closed: bool,
    next_ipv4_identification: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum LinuxFrameDisposition {
    IgnoredOutgoing,
    Parsed(FrameDisposition),
}

fn classify_received_frame(
    frame: &[u8],
    packet_type: libc::c_uchar,
    config: &RawEthernetConfig,
) -> LinuxFrameDisposition {
    if packet_type == libc::PACKET_OUTGOING || packet_type == libc::PACKET_LOOPBACK {
        return LinuxFrameDisposition::IgnoredOutgoing;
    }
    LinuxFrameDisposition::Parsed(parse_udp_frame(frame, config))
}

fn saturating_increment(counter: &mut u64) {
    *counter = counter.saturating_add(1);
}

fn record_packet_type(statistics: &mut TransportStatistics, packet_type: libc::c_uchar) {
    let counters = &mut statistics.frame_classification;
    let counter = match packet_type {
        libc::PACKET_HOST => &mut counters.packet_host_frames,
        libc::PACKET_BROADCAST => &mut counters.packet_broadcast_frames,
        libc::PACKET_MULTICAST => &mut counters.packet_multicast_frames,
        libc::PACKET_OTHERHOST => &mut counters.packet_other_host_frames,
        libc::PACKET_OUTGOING => &mut counters.packet_outgoing_frames,
        libc::PACKET_LOOPBACK => &mut counters.packet_loopback_frames,
        _ => &mut counters.packet_unknown_frames,
    };
    saturating_increment(counter);
}

fn record_parse_error(counters: &mut FrameClassificationStatistics, error: &EthernetError) {
    let counter = match error {
        EthernetError::FrameTooShort(size) if *size < ETHERNET_HEADER_BYTES => {
            &mut counters.ethernet_header_too_short_frames
        }
        EthernetError::UnsupportedEtherType(_) => &mut counters.unsupported_ethertype_frames,
        EthernetError::FrameTooShort(_) => &mut counters.frame_too_short_failures,
        EthernetError::InvalidIpv4Header | EthernetError::PayloadTooLarge(_) => {
            &mut counters.invalid_ipv4_header_failures
        }
        EthernetError::FragmentedIpv4 => &mut counters.fragmented_ipv4_failures,
        EthernetError::InvalidIpv4Checksum => &mut counters.invalid_ipv4_checksum_failures,
        EthernetError::InvalidIpv6Header => &mut counters.invalid_ipv6_header_failures,
        EthernetError::UnsupportedIpv6ExtensionHeader(_) => {
            &mut counters.unsupported_ipv6_extension_header_failures
        }
        EthernetError::InvalidIpv6PayloadLength => {
            &mut counters.invalid_ipv6_payload_length_failures
        }
        EthernetError::MissingIpv6UdpChecksum => &mut counters.missing_ipv6_udp_checksum_failures,
        EthernetError::InvalidIpv6UdpChecksum => &mut counters.invalid_ipv6_udp_checksum_failures,
        EthernetError::InvalidUdpLength => &mut counters.invalid_udp_length_failures,
        EthernetError::MissingIpv4UdpChecksum | EthernetError::InvalidUdpChecksum => {
            &mut counters.invalid_udp_checksum_failures
        }
        _ => return,
    };
    saturating_increment(counter);
}

fn record_parsed_frame(statistics: &mut TransportStatistics, disposition: &FrameDisposition) {
    match disposition {
        FrameDisposition::Matched { .. } => {
            saturating_increment(&mut statistics.received_frames);
        }
        FrameDisposition::Foreign => {
            saturating_increment(&mut statistics.foreign_frames);
            saturating_increment(&mut statistics.frame_classification.endpoint_mismatch_frames);
        }
        FrameDisposition::Invalid(error @ EthernetError::UnsupportedEtherType(_)) => {
            saturating_increment(&mut statistics.ignored_non_ipv4_frames);
            record_parse_error(&mut statistics.frame_classification, error);
        }
        FrameDisposition::Invalid(error) => {
            saturating_increment(&mut statistics.invalid_frames);
            record_parse_error(&mut statistics.frame_classification, error);
        }
    }
}

fn received_packet_type(
    address: &libc::sockaddr_ll,
    address_length: libc::socklen_t,
    expected_interface_index: i32,
) -> Result<libc::c_uchar, TransportError> {
    if (address_length as usize) < mem::size_of::<libc::sockaddr_ll>() {
        return Err(other(format!(
            "AF_PACKET receive returned short sockaddr_ll length {address_length}"
        )));
    }
    if address.sll_family != libc::AF_PACKET as libc::c_ushort {
        return Err(other(format!(
            "AF_PACKET receive returned family {}",
            address.sll_family
        )));
    }
    if address.sll_ifindex != expected_interface_index {
        return Err(other(format!(
            "AF_PACKET receive returned interface index {}; expected {expected_interface_index}",
            address.sll_ifindex
        )));
    }
    Ok(address.sll_pkttype)
}

impl LinuxRawEthernetTransport {
    pub fn open(config: RawEthernetConfig) -> Result<Self, TransportError> {
        config
            .validate()
            .map_err(|error| other(error.to_string()))?;
        let snapshot = inspect_interface(config.interface_name())?;
        snapshot.validate_for(&config)?;

        let protocol = i32::from((libc::ETH_P_ALL as u16).to_be());
        // SAFETY: socket arguments are Linux AF_PACKET constants and no
        // pointers cross this call.
        let descriptor = unsafe {
            libc::socket(
                libc::AF_PACKET,
                libc::SOCK_RAW | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                protocol,
            )
        };
        if descriptor < 0 {
            return Err(last_os_error(
                "opening AF_PACKET socket (CAP_NET_RAW is required)",
            ));
        }
        let socket = PacketSocket(descriptor);
        let address = libc::sockaddr_ll {
            sll_family: libc::AF_PACKET as libc::c_ushort,
            sll_protocol: (libc::ETH_P_ALL as u16).to_be(),
            sll_ifindex: snapshot.index,
            sll_hatype: 0,
            sll_pkttype: 0,
            sll_halen: 6,
            sll_addr: {
                let mut bytes = [0_u8; 8];
                bytes[..6].copy_from_slice(&config.host().mac().octets());
                bytes
            },
        };
        // SAFETY: `address` is a fully initialized sockaddr_ll and the size
        // passed to bind matches its concrete type.
        let bind_result = unsafe {
            libc::bind(
                socket.0,
                (&raw const address).cast::<libc::sockaddr>(),
                mem::size_of::<libc::sockaddr_ll>() as libc::socklen_t,
            )
        };
        if bind_result != 0 {
            return Err(last_os_error("binding AF_PACKET socket"));
        }

        let enabled: libc::c_int = 1;
        // SAFETY: option storage is a live initialized integer of the stated size.
        if unsafe {
            libc::setsockopt(
                socket.0,
                libc::SOL_PACKET,
                libc::PACKET_AUXDATA,
                (&raw const enabled).cast(),
                mem::size_of_val(&enabled) as libc::socklen_t,
            )
        } != 0
        {
            return Err(last_os_error("enabling PACKET_AUXDATA"));
        }

        let receive_buffer_bytes = socket_receive_buffer(socket.0)?;
        let ring_capacity = config.ring_capacity();
        Ok(Self {
            socket,
            config,
            interface_index: snapshot.index,
            ring: BoundedPacketRing::new(ring_capacity)?,
            statistics: TransportStatistics {
                queue_capacity: ring_capacity,
                receive_buffer_bytes: Some(receive_buffer_bytes),
                ..TransportStatistics::default()
            },
            receive_ready: false,
            closed: false,
            next_ipv4_identification: 0,
        })
    }

    pub fn interface_snapshot(&self) -> Result<InterfaceSnapshot, TransportError> {
        inspect_interface(self.config.interface_name())
    }

    fn drain_socket(&mut self, deadline: Instant) -> Result<DrainPassOutcome, TransportError> {
        let mut buffer = [0_u8; MAXIMUM_FRAME_BYTES];
        let outcome = drain_pass(deadline, Instant::now, || {
            // Leave packets in the kernel until the consumer frees storage.
            if self.ring.len() == self.ring.capacity() {
                return Ok(false);
            }
            let mut address: libc::sockaddr_ll = unsafe { mem::zeroed() };
            // usize alignment is sufficient for cmsghdr on supported Linux targets.
            let mut control = [0_usize; 16];
            let mut iovec = libc::iovec {
                iov_base: buffer.as_mut_ptr().cast(),
                iov_len: buffer.len(),
            };
            let mut message: libc::msghdr = unsafe { mem::zeroed() };
            message.msg_name = (&raw mut address).cast();
            message.msg_namelen = mem::size_of::<libc::sockaddr_ll>() as libc::socklen_t;
            message.msg_iov = &raw mut iovec;
            message.msg_iovlen = 1;
            message.msg_control = control.as_mut_ptr().cast();
            message.msg_controllen = mem::size_of_val(&control);
            // SAFETY: `buffer` and `address` are writable for their full
            // reported lengths, and the socket is a live nonblocking
            // descriptor owned by this transport.
            let received = unsafe {
                libc::recvmsg(
                    self.socket.0,
                    &raw mut message,
                    libc::MSG_DONTWAIT | libc::MSG_TRUNC,
                )
            };
            if received < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::WouldBlock {
                    return Ok(false);
                }
                if error.kind() == io::ErrorKind::Interrupted {
                    return Ok(true);
                }
                return Err(TransportError::from_io("receive", error));
            }
            if received == 0 {
                return Ok(false);
            }
            if received as usize > buffer.len()
                || message.msg_flags & (libc::MSG_TRUNC | libc::MSG_CTRUNC) != 0
            {
                saturating_increment(&mut self.statistics.truncated_frames);
                return Ok(true);
            }
            let packet_type =
                received_packet_type(&address, message.msg_namelen, self.interface_index)?;
            record_packet_type(&mut self.statistics, packet_type);
            let mut tagged = false;
            // SAFETY: recvmsg populated this live, aligned control buffer. Check
            // the ancillary length before reading its payload, which may be unaligned.
            unsafe {
                let mut header = libc::CMSG_FIRSTHDR(&message);
                while !header.is_null() {
                    if (*header).cmsg_level == libc::SOL_PACKET
                        && (*header).cmsg_type == libc::PACKET_AUXDATA
                        && (*header).cmsg_len
                            >= libc::CMSG_LEN(mem::size_of::<libc::tpacket_auxdata>() as u32)
                                as usize
                    {
                        let aux = ptr::read_unaligned(
                            libc::CMSG_DATA(header).cast::<libc::tpacket_auxdata>(),
                        );
                        tagged |= aux.tp_status & libc::TP_STATUS_VLAN_VALID != 0;
                    }
                    header = libc::CMSG_NXTHDR(&message, header);
                }
            }
            if tagged
                || (received >= 14
                    && matches!(
                        u16::from_be_bytes([buffer[12], buffer[13]]),
                        0x8100 | 0x88a8
                    ))
            {
                saturating_increment(&mut self.statistics.vlan_frames);
                return Ok(true);
            }
            match classify_received_frame(&buffer[..received as usize], packet_type, &self.config) {
                LinuxFrameDisposition::IgnoredOutgoing => {
                    saturating_increment(&mut self.statistics.ignored_outgoing_frames);
                }
                LinuxFrameDisposition::Parsed(disposition) => {
                    record_parsed_frame(&mut self.statistics, &disposition);
                    if let FrameDisposition::Matched { payload, sender } = disposition {
                        self.ring.push(ReceivedFrame { payload, sender });
                    }
                }
            }
            Ok(true)
        })?;
        self.statistics.dropped_frames = self.ring.dropped();
        self.statistics.maximum_queue_depth = self.ring.maximum_depth();
        if self.refresh_kernel_statistics().is_err() {
            saturating_increment(&mut self.statistics.statistics_failures);
        }
        Ok(outcome)
    }

    fn refresh_kernel_statistics(&mut self) -> Result<(), TransportError> {
        let mut packet_statistics = PacketStatistics::default();
        let mut length = mem::size_of::<PacketStatistics>() as libc::socklen_t;
        // SAFETY: output storage and its length are valid. Linux resets these
        // counters after a successful PACKET_STATISTICS read, so we accumulate
        // the reported drop count.
        let result = unsafe {
            libc::getsockopt(
                self.socket.0,
                libc::SOL_PACKET,
                PACKET_STATISTICS,
                (&raw mut packet_statistics).cast(),
                &mut length,
            )
        };
        if result != 0 {
            return Err(last_os_error("reading PACKET_STATISTICS"));
        }
        self.statistics.kernel_dropped_frames = self
            .statistics
            .kernel_dropped_frames
            .saturating_add(u64::from(packet_statistics.tp_drops));
        Ok(())
    }

    fn close_with(
        &mut self,
        close_socket: impl FnOnce(RawFd) -> io::Result<()>,
    ) -> Result<(), TransportError> {
        if self.closed {
            return Ok(());
        }
        let statistics_result = if self.receive_ready {
            self.refresh_kernel_statistics()
        } else {
            Ok(())
        };
        let close_result = self
            .socket
            .close_with(close_socket)
            .map_err(|error| TransportError::from_io("closing AF_PACKET socket", error));
        self.closed = true;
        self.receive_ready = false;
        self.statistics.receive_ready = false;
        statistics_result.and(close_result)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DrainPassOutcome {
    Drained,
    DeadlineExpired,
    BudgetExhausted,
}

fn drain_pass(
    deadline: Instant,
    mut now: impl FnMut() -> Instant,
    mut receive_one: impl FnMut() -> Result<bool, TransportError>,
) -> Result<DrainPassOutcome, TransportError> {
    for _ in 0..RECEIVE_DRAIN_PACKET_BUDGET {
        if now() >= deadline {
            return Ok(DrainPassOutcome::DeadlineExpired);
        }
        if !receive_one()? {
            return Ok(DrainPassOutcome::Drained);
        }
    }
    Ok(DrainPassOutcome::BudgetExhausted)
}

impl Transport for LinuxRawEthernetTransport {
    fn start_receive(&mut self) -> Result<(), TransportError> {
        if self.closed {
            return Err(TransportError::Closed);
        }
        // A failed revalidation must not leave a previously ready socket usable.
        self.receive_ready = false;
        self.statistics.receive_ready = false;
        let current = inspect_interface(self.config.interface_name())?;
        current.validate_for(&self.config)?;
        if current.index != self.interface_index {
            return Err(other(
                "interface index changed since socket binding".to_owned(),
            ));
        }
        self.receive_ready = true;
        self.statistics.receive_ready = true;
        Ok(())
    }

    fn send(&mut self, payload: &[u8]) -> Result<(), TransportError> {
        if self.closed {
            return Err(TransportError::Closed);
        }
        if !self.receive_ready {
            return Err(TransportError::NotReady);
        }
        let options = if self.config.host().network().as_ipv4().is_some() {
            let identification = self.next_ipv4_identification;
            self.next_ipv4_identification = self.next_ipv4_identification.wrapping_add(1);
            IpPacketOptions::Ipv4 { identification }
        } else {
            IpPacketOptions::Ipv6
        };
        let frame = build_udp_frame(&self.config, payload, options)
            .map_err(|error| other(error.to_string()))?;
        // SAFETY: `frame` remains live and immutable for the duration of send.
        let sent = unsafe {
            libc::send(
                self.socket.0,
                frame.as_ptr().cast(),
                frame.len(),
                libc::MSG_NOSIGNAL,
            )
        };
        if sent < 0 {
            return Err(last_os_error("sending AF_PACKET frame"));
        }
        if sent as usize != frame.len() {
            return Err(other(format!(
                "short AF_PACKET send: wrote {sent} of {} bytes",
                frame.len()
            )));
        }
        self.statistics.sent_frames = self.statistics.sent_frames.saturating_add(1);
        Ok(())
    }

    fn receive(&mut self, deadline: Instant) -> Result<ReceivedFrame, TransportError> {
        if self.closed {
            return Err(TransportError::Closed);
        }
        if !self.receive_ready {
            return Err(TransportError::NotReady);
        }
        loop {
            if Instant::now() >= deadline {
                saturating_increment(&mut self.statistics.receive_timeouts);
                return Err(TransportError::TimedOut);
            }
            if let Some(frame) = self.ring.pop() {
                return Ok(frame);
            }
            if self.drain_socket(deadline)? == DrainPassOutcome::DeadlineExpired
                || Instant::now() >= deadline
            {
                self.statistics.receive_timeouts =
                    self.statistics.receive_timeouts.saturating_add(1);
                return Err(TransportError::TimedOut);
            }
            if let Some(frame) = self.ring.pop() {
                return Ok(frame);
            }
            let now = Instant::now();
            let timeout = poll_timeout_milliseconds(now, deadline);
            let mut descriptor = libc::pollfd {
                fd: self.socket.0,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: the pollfd points to one initialized descriptor.
            let result = unsafe { libc::poll(&mut descriptor, 1, timeout) };
            if result < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(TransportError::from_io("poll", error));
            }
            if result == 0 {
                self.statistics.receive_timeouts =
                    self.statistics.receive_timeouts.saturating_add(1);
                return Err(TransportError::TimedOut);
            }
            if descriptor.revents & libc::POLLIN != 0 {
                continue;
            } else if descriptor.revents != 0 {
                return Err(other(format!(
                    "AF_PACKET poll returned terminal events 0x{:x}",
                    descriptor.revents
                )));
            }
        }
    }

    fn statistics(&self) -> TransportStatistics {
        self.statistics
    }

    fn close(&mut self) -> Result<(), TransportError> {
        self.close_with(close_descriptor)
    }
}

#[derive(Default)]
#[repr(C)]
struct PacketStatistics {
    tp_packets: u32,
    tp_drops: u32,
}

fn close_descriptor(descriptor: RawFd) -> io::Result<()> {
    // SAFETY: the descriptor is owned by the caller and invalidated before this call.
    if unsafe { libc::close(descriptor) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn socket_receive_buffer(descriptor: RawFd) -> Result<usize, TransportError> {
    let mut value: libc::c_int = 0;
    let mut length = mem::size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: output storage and its length match SO_RCVBUF's integer value.
    let result = unsafe {
        libc::getsockopt(
            descriptor,
            libc::SOL_SOCKET,
            libc::SO_RCVBUF,
            (&raw mut value).cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(last_os_error("reading SO_RCVBUF"));
    }
    usize::try_from(value).map_err(|_| other(format!("SO_RCVBUF returned {value}")))
}

fn poll_timeout_milliseconds(now: Instant, deadline: Instant) -> libc::c_int {
    let nanos = deadline.saturating_duration_since(now).as_nanos();
    let milliseconds = nanos.div_ceil(1_000_000).max(1);
    i32::try_from(milliseconds).unwrap_or(i32::MAX)
}

fn last_os_error(context: &'static str) -> TransportError {
    TransportError::from_io(context, io::Error::last_os_error())
}

fn other(message: String) -> TransportError {
    TransportError::Other(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[rustfmt::skip]
    use crate::{
        Endpoint,
        IpPacketOptions,
        RAW_ETHERNET_CONFIG_SCHEMA_VERSION,
        RawEthernetEndpoint,
    };
    use std::net::Ipv4Addr;

    const ETHERNET_HEADER_BYTES: usize = 14;

    fn config() -> RawEthernetConfig {
        RawEthernetConfig::new(
            RAW_ETHERNET_CONFIG_SCHEMA_VERSION,
            "eth0",
            RawEthernetEndpoint::new(
                MacAddress::new([0x02, 0, 0, 0, 0, 1]),
                Endpoint::new(Ipv4Addr::new(169, 254, 209, 1), 49_152).expect("host"),
            )
            .expect("host endpoint"),
            RawEthernetEndpoint::new(
                MacAddress::new([0x02, 0, 0, 0, 0, 0x7a]),
                Endpoint::new(Ipv4Addr::new(169, 254, 209, 0), 24_576).expect("board"),
            )
            .expect("board endpoint"),
            8,
        )
        .expect("config")
    }

    fn board_to_host_frame(payload: &[u8]) -> Vec<u8> {
        let config = config();
        let reverse = RawEthernetConfig::new(
            RAW_ETHERNET_CONFIG_SCHEMA_VERSION,
            config.interface_name(),
            config.board(),
            config.host(),
            config.ring_capacity(),
        )
        .expect("reverse config");
        build_udp_frame(
            &reverse,
            payload,
            IpPacketOptions::Ipv4 { identification: 7 },
        )
        .expect("board-to-host frame")
    }

    fn packet_address() -> libc::sockaddr_ll {
        libc::sockaddr_ll {
            sll_family: libc::AF_PACKET as libc::c_ushort,
            sll_protocol: 0,
            sll_ifindex: 2,
            sll_hatype: libc::ARPHRD_ETHER,
            sll_pkttype: 0,
            sll_halen: 6,
            sll_addr: [0x02, 0, 0, 0, 0, 1, 0, 0],
        }
    }

    #[test]
    fn packet_metadata_requires_ethernet_six_bytes_and_positive_index() {
        let valid = packet_address();
        assert_eq!(
            ethernet_packet_metadata("eth0", &valid).expect("valid Ethernet metadata"),
            (2, MacAddress::new([0x02, 0, 0, 0, 0, 1]))
        );

        let mut wrong_type = valid;
        wrong_type.sll_hatype = libc::ARPHRD_LOOPBACK;
        assert!(ethernet_packet_metadata("eth0", &wrong_type).is_err());

        let mut wrong_length = valid;
        wrong_length.sll_halen = 8;
        assert!(ethernet_packet_metadata("eth0", &wrong_length).is_err());

        let mut wrong_index = valid;
        wrong_index.sll_ifindex = 0;
        assert!(ethernet_packet_metadata("eth0", &wrong_index).is_err());
    }

    #[test]
    fn receive_metadata_requires_complete_packet_identity() {
        let valid = packet_address();
        let length = mem::size_of::<libc::sockaddr_ll>() as libc::socklen_t;
        assert_eq!(
            received_packet_type(&valid, length, 2).expect("valid receive metadata"),
            libc::PACKET_HOST
        );
        assert!(received_packet_type(&valid, length - 1, 2).is_err());

        let mut wrong_family = valid;
        wrong_family.sll_family = libc::AF_INET as libc::c_ushort;
        assert!(received_packet_type(&wrong_family, length, 2).is_err());

        let mut wrong_index = valid;
        wrong_index.sll_ifindex = 3;
        assert!(received_packet_type(&wrong_index, length, 2).is_err());

        let mut unknown_type = valid;
        unknown_type.sll_pkttype = libc::PACKET_LOOPBACK + 1;
        assert_eq!(
            received_packet_type(&unknown_type, length, 2).expect("unknown type is counted"),
            libc::PACKET_LOOPBACK + 1
        );
    }

    #[test]
    fn receive_classifier_separates_ignored_and_integrity_frames() {
        let config = config();
        let matched = board_to_host_frame(b"telemetry");
        for packet_type in [libc::PACKET_OUTGOING, libc::PACKET_LOOPBACK] {
            assert_eq!(
                classify_received_frame(&matched, packet_type, &config),
                LinuxFrameDisposition::IgnoredOutgoing
            );
        }

        for ether_type in [0x0806_u16, 0x86dd] {
            let mut link_frame = vec![0_u8; 60];
            link_frame[12..14].copy_from_slice(&ether_type.to_be_bytes());
            assert!(matches!(
                classify_received_frame(&link_frame, libc::PACKET_HOST, &config),
                LinuxFrameDisposition::Parsed(FrameDisposition::Invalid(
                    EthernetError::UnsupportedEtherType(value)
                )) if value == ether_type
            ));
        }

        assert!(matches!(
            classify_received_frame(&matched, libc::PACKET_HOST, &config),
            LinuxFrameDisposition::Parsed(FrameDisposition::Matched { payload, .. })
                if payload == b"telemetry"
        ));

        let mut foreign = matched.clone();
        foreign[6] ^= 1;
        assert_eq!(
            classify_received_frame(&foreign, libc::PACKET_HOST, &config),
            LinuxFrameDisposition::Parsed(FrameDisposition::Foreign)
        );

        let mut malformed = matched;
        malformed[24] ^= 1;
        assert!(matches!(
            classify_received_frame(&malformed, libc::PACKET_HOST, &config),
            LinuxFrameDisposition::Parsed(FrameDisposition::Invalid(
                EthernetError::InvalidIpv4Checksum
            ))
        ));
        for short_length in [8, ETHERNET_HEADER_BYTES, MINIMUM_IPV4_UDP_FRAME_BYTES - 1] {
            let mut short = vec![0_u8; short_length];
            if short_length >= ETHERNET_HEADER_BYTES {
                short[12..14].copy_from_slice(&0x0806_u16.to_be_bytes());
            }
            assert!(matches!(
                classify_received_frame(&short, libc::PACKET_HOST, &config),
                LinuxFrameDisposition::Parsed(FrameDisposition::Invalid(_))
            ));
        }
    }

    #[test]
    fn packet_type_accounting_counts_known_and_unknown_values() {
        let mut statistics = TransportStatistics::default();
        for packet_type in [
            libc::PACKET_HOST,
            libc::PACKET_BROADCAST,
            libc::PACKET_MULTICAST,
            libc::PACKET_OTHERHOST,
            libc::PACKET_OUTGOING,
            libc::PACKET_LOOPBACK,
            libc::PACKET_LOOPBACK + 1,
        ] {
            record_packet_type(&mut statistics, packet_type);
        }

        assert_eq!(statistics.frame_classification.packet_host_frames, 1);
        assert_eq!(statistics.frame_classification.packet_broadcast_frames, 1);
        assert_eq!(statistics.frame_classification.packet_multicast_frames, 1);
        assert_eq!(statistics.frame_classification.packet_other_host_frames, 1);
        assert_eq!(statistics.frame_classification.packet_outgoing_frames, 1);
        assert_eq!(statistics.frame_classification.packet_loopback_frames, 1);
        assert_eq!(statistics.frame_classification.packet_unknown_frames, 1);

        statistics.frame_classification.packet_host_frames = u64::MAX;
        record_packet_type(&mut statistics, libc::PACKET_HOST);
        assert_eq!(statistics.frame_classification.packet_host_frames, u64::MAX);
    }

    #[test]
    fn parsed_frame_accounting_preserves_unsupported_ethertype_aggregate() {
        let mut statistics = TransportStatistics::default();
        record_parsed_frame(
            &mut statistics,
            &FrameDisposition::Invalid(EthernetError::UnsupportedEtherType(0x0806)),
        );

        assert_eq!(statistics.ignored_non_ipv4_frames, 1);
        assert_eq!(
            statistics.frame_classification.unsupported_ethertype_frames,
            1
        );
        assert_eq!(statistics.invalid_frames, 0);
        assert_eq!(statistics.received_frames, 0);

        statistics.ignored_non_ipv4_frames = u64::MAX;
        statistics.frame_classification.unsupported_ethertype_frames = u64::MAX;
        record_parsed_frame(
            &mut statistics,
            &FrameDisposition::Invalid(EthernetError::UnsupportedEtherType(0x86dd)),
        );
        assert_eq!(statistics.ignored_non_ipv4_frames, u64::MAX);
        assert_eq!(
            statistics.frame_classification.unsupported_ethertype_frames,
            u64::MAX
        );
        assert_eq!(statistics.invalid_frames, 0);
    }

    #[test]
    fn parsed_frame_accounting_keeps_real_parse_failures_invalid() {
        let mut statistics = TransportStatistics::default();
        record_parsed_frame(
            &mut statistics,
            &FrameDisposition::Invalid(EthernetError::InvalidIpv4Checksum),
        );

        assert_eq!(statistics.invalid_frames, 1);
        assert_eq!(
            statistics
                .frame_classification
                .invalid_ipv4_checksum_failures,
            1
        );
        assert_eq!(statistics.ignored_non_ipv4_frames, 0);
    }

    #[test]
    fn parse_error_accounting_records_factual_reasons_and_saturates() {
        let mut counters = FrameClassificationStatistics::default();
        record_parse_error(&mut counters, &EthernetError::FrameTooShort(8));
        assert_eq!(counters.ethernet_header_too_short_frames, 1);

        let mut counters = FrameClassificationStatistics::default();
        record_parse_error(&mut counters, &EthernetError::UnsupportedEtherType(0x0806));
        assert_eq!(counters.unsupported_ethertype_frames, 1);

        let mut counters = FrameClassificationStatistics::default();
        record_parse_error(
            &mut counters,
            &EthernetError::FrameTooShort(ETHERNET_HEADER_BYTES),
        );
        assert_eq!(counters.frame_too_short_failures, 1);

        let mut counters = FrameClassificationStatistics::default();
        record_parse_error(&mut counters, &EthernetError::InvalidIpv4Header);
        assert_eq!(counters.invalid_ipv4_header_failures, 1);

        let mut counters = FrameClassificationStatistics::default();
        record_parse_error(&mut counters, &EthernetError::FragmentedIpv4);
        assert_eq!(counters.fragmented_ipv4_failures, 1);

        let mut counters = FrameClassificationStatistics::default();
        record_parse_error(&mut counters, &EthernetError::InvalidIpv4Checksum);
        assert_eq!(counters.invalid_ipv4_checksum_failures, 1);

        let mut counters = FrameClassificationStatistics::default();
        record_parse_error(&mut counters, &EthernetError::InvalidIpv6Header);
        assert_eq!(counters.invalid_ipv6_header_failures, 1);

        let mut counters = FrameClassificationStatistics::default();
        record_parse_error(
            &mut counters,
            &EthernetError::UnsupportedIpv6ExtensionHeader(0),
        );
        assert_eq!(counters.unsupported_ipv6_extension_header_failures, 1);

        let mut counters = FrameClassificationStatistics::default();
        record_parse_error(&mut counters, &EthernetError::InvalidIpv6PayloadLength);
        assert_eq!(counters.invalid_ipv6_payload_length_failures, 1);

        let mut counters = FrameClassificationStatistics::default();
        record_parse_error(&mut counters, &EthernetError::MissingIpv6UdpChecksum);
        assert_eq!(counters.missing_ipv6_udp_checksum_failures, 1);

        let mut counters = FrameClassificationStatistics::default();
        record_parse_error(&mut counters, &EthernetError::InvalidIpv6UdpChecksum);
        assert_eq!(counters.invalid_ipv6_udp_checksum_failures, 1);

        let mut counters = FrameClassificationStatistics::default();
        record_parse_error(&mut counters, &EthernetError::InvalidUdpLength);
        assert_eq!(counters.invalid_udp_length_failures, 1);

        let mut counters = FrameClassificationStatistics::default();
        record_parse_error(&mut counters, &EthernetError::InvalidUdpChecksum);
        assert_eq!(counters.invalid_udp_checksum_failures, 1);

        let mut counters = FrameClassificationStatistics {
            invalid_ipv4_checksum_failures: u64::MAX,
            ..FrameClassificationStatistics::default()
        };
        record_parse_error(&mut counters, &EthernetError::InvalidIpv4Checksum);
        assert_eq!(counters.invalid_ipv4_checksum_failures, u64::MAX);
    }

    #[test]
    fn drain_pass_honors_deadline_and_packet_budget() {
        let deadline = Instant::now() + std::time::Duration::from_secs(1);
        let mut received = 0;
        assert_eq!(
            drain_pass(
                deadline,
                Instant::now,
                || -> Result<bool, TransportError> {
                    received += 1;
                    Ok(true)
                },
            )
            .expect("bounded pass"),
            DrainPassOutcome::BudgetExhausted
        );
        assert_eq!(received, RECEIVE_DRAIN_PACKET_BUDGET);

        let start = Instant::now();
        let deadline = start + std::time::Duration::from_millis(1);
        let mut times = [start, deadline].into_iter();
        let mut received = 0;
        assert_eq!(
            drain_pass(
                deadline,
                || times.next().expect("deadline checked before each receive"),
                || -> Result<bool, TransportError> {
                    received += 1;
                    Ok(true)
                },
            )
            .expect("deadline pass"),
            DrainPassOutcome::DeadlineExpired
        );
        assert_eq!(received, 1);

        let mut received = 0;
        assert_eq!(
            drain_pass(
                deadline,
                || deadline,
                || -> Result<bool, TransportError> {
                    received += 1;
                    Ok(true)
                },
            )
            .expect("expired pass"),
            DrainPassOutcome::DeadlineExpired
        );
        assert_eq!(received, 0);
    }

    fn transport_with_descriptor(
        descriptor: RawFd,
        receive_ready: bool,
    ) -> LinuxRawEthernetTransport {
        let config = config();
        LinuxRawEthernetTransport {
            socket: PacketSocket(descriptor),
            interface_index: 2,
            ring: BoundedPacketRing::new(config.ring_capacity()).expect("ring"),
            statistics: TransportStatistics {
                receive_ready,
                queue_capacity: config.ring_capacity(),
                ..TransportStatistics::default()
            },
            config,
            receive_ready,
            closed: false,
            next_ipv4_identification: 0,
        }
    }

    #[test]
    fn diagnostic_refresh_failure_does_not_discard_a_buffered_packet() {
        let mut transport = transport_with_descriptor(i32::MAX, true);
        let frame = ReceivedFrame {
            payload: vec![1],
            sender: transport.config.board().network(),
        };
        // Fill the ring so draining does not touch the injected invalid socket.
        for _ in 0..transport.ring.capacity() {
            transport.ring.push(frame.clone());
        }
        transport
            .drain_socket(Instant::now() + std::time::Duration::from_secs(1))
            .unwrap();
        assert_eq!(transport.statistics.statistics_failures, 1);
        assert_eq!(transport.ring.pop(), Some(frame));
        transport.socket.0 = -1;
    }

    #[test]
    fn close_invalidates_socket_once_and_closes_after_errors() {
        let mut transport = transport_with_descriptor(i32::MAX, true);
        let mut close_calls = 0;
        let error = transport
            .close_with(|_| {
                close_calls += 1;
                Err(io::Error::other("injected close failure"))
            })
            .expect_err("statistics failure wins");
        assert!(error.to_string().contains("PACKET_STATISTICS"));
        assert_eq!(close_calls, 1);
        assert_eq!(transport.socket.0, -1);
        assert!(transport.closed);
        assert!(!transport.receive_ready);
        assert!(!transport.statistics.receive_ready);

        transport
            .close_with(|_| {
                close_calls += 1;
                Ok(())
            })
            .expect("second close is idempotent");
        assert_eq!(close_calls, 1);
        assert_eq!(transport.start_receive(), Err(TransportError::Closed));
        assert_eq!(transport.send(&[]), Err(TransportError::Closed));
        assert_eq!(
            transport.receive(Instant::now()),
            Err(TransportError::Closed)
        );

        let mut transport = transport_with_descriptor(i32::MAX, false);
        let error = transport
            .close_with(|_| Err(io::Error::other("injected close failure")))
            .expect_err("close failure");
        assert!(error.to_string().contains("injected close failure"));
        assert!(transport.closed);
        assert_eq!(transport.socket.0, -1);
    }
}
