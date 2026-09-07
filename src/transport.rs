use std::collections::VecDeque;
use std::error::Error;
use std::fmt;
use std::time::Instant;

use crate::Endpoint;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReceivedFrame {
    pub payload: Vec<u8>,
    pub sender: Endpoint,
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FrameClassificationStatistics {
    pub packet_host_frames: u64,
    pub packet_broadcast_frames: u64,
    pub packet_multicast_frames: u64,
    pub packet_other_host_frames: u64,
    pub packet_outgoing_frames: u64,
    pub packet_loopback_frames: u64,
    pub packet_unknown_frames: u64,
    pub ethernet_header_too_short_frames: u64,
    pub unsupported_ethertype_frames: u64,
    pub endpoint_mismatch_frames: u64,
    pub frame_too_short_failures: u64,
    pub invalid_ipv4_header_failures: u64,
    pub fragmented_ipv4_failures: u64,
    pub invalid_ipv4_checksum_failures: u64,
    pub invalid_ipv6_header_failures: u64,
    pub unsupported_ipv6_extension_header_failures: u64,
    pub invalid_ipv6_payload_length_failures: u64,
    pub missing_ipv6_udp_checksum_failures: u64,
    pub invalid_ipv6_udp_checksum_failures: u64,
    pub invalid_udp_length_failures: u64,
    pub invalid_udp_checksum_failures: u64,
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TransportStatistics {
    /// Failed diagnostic reads. These never invalidate received packets.
    pub statistics_failures: u64,
    pub truncated_frames: u64,
    pub vlan_frames: u64,
    pub receive_ready: bool,
    pub sent_frames: u64,
    pub received_frames: u64,
    pub receive_timeouts: u64,
    pub dropped_frames: u64,
    pub kernel_dropped_frames: u64,
    pub ignored_outgoing_frames: u64,
    pub ignored_non_ipv4_frames: u64,
    pub invalid_frames: u64,
    pub foreign_frames: u64,
    pub frame_classification: FrameClassificationStatistics,
    pub maximum_queue_depth: usize,
    pub queue_capacity: usize,
    pub receive_buffer_bytes: Option<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransportError {
    NotReady,
    TimedOut,
    Closed,
    /// An OS failure with stable operation and errno evidence.
    Io {
        operation: &'static str,
        kind: std::io::ErrorKind,
        raw_os_error: Option<i32>,
        message: String,
    },
    Other(String),
}

impl fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotReady => formatter.write_str("receive path is not ready"),
            Self::TimedOut => formatter.write_str("receive deadline expired"),
            Self::Closed => formatter.write_str("transport is closed"),
            Self::Io {
                operation, message, ..
            } => write!(formatter, "{operation}: {message}"),
            Self::Other(message) => formatter.write_str(message),
        }
    }
}

impl Error for TransportError {}

impl TransportError {
    pub fn from_io(operation: &'static str, error: std::io::Error) -> Self {
        Self::Io {
            operation,
            kind: error.kind(),
            raw_os_error: error.raw_os_error(),
            message: error.to_string(),
        }
    }
}

pub trait Transport {
    fn start_receive(&mut self) -> Result<(), TransportError>;
    fn send(&mut self, payload: &[u8]) -> Result<(), TransportError>;
    fn receive(&mut self, deadline: Instant) -> Result<ReceivedFrame, TransportError>;
    fn statistics(&self) -> TransportStatistics;
    fn close(&mut self) -> Result<(), TransportError>;
}

#[derive(Debug)]
pub struct BoundedPacketRing<T> {
    frames: VecDeque<T>,
    capacity: usize,
    dropped: u64,
    maximum_depth: usize,
}

impl<T> BoundedPacketRing<T> {
    pub fn new(capacity: usize) -> Result<Self, TransportError> {
        if capacity == 0 {
            return Err(TransportError::Other(
                "packet ring capacity must be at least 1".to_owned(),
            ));
        }
        let mut frames = VecDeque::new();
        frames.try_reserve_exact(capacity).map_err(|error| {
            TransportError::Other(format!("cannot allocate packet ring: {error}"))
        })?;
        Ok(Self {
            frames,
            capacity,
            dropped: 0,
            maximum_depth: 0,
        })
    }

    pub fn push(&mut self, frame: T) -> bool {
        if self.frames.len() == self.capacity {
            self.dropped = self.dropped.saturating_add(1);
            return false;
        }
        self.frames.push_back(frame);
        self.maximum_depth = self.maximum_depth.max(self.frames.len());
        true
    }

    pub fn pop(&mut self) -> Option<T> {
        self.frames.pop_front()
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    pub const fn dropped(&self) -> u64 {
        self.dropped
    }

    pub const fn maximum_depth(&self) -> usize {
        self.maximum_depth
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryTransportEvent {
    ReceiveStarted,
    Sent,
    Received,
    TimedOut,
    Closed,
}

#[derive(Debug, Default)]
pub struct MemoryTransport {
    receive_ready: bool,
    closed: bool,
    incoming: VecDeque<Result<ReceivedFrame, TransportError>>,
    sent: Vec<Vec<u8>>,
    events: Vec<MemoryTransportEvent>,
    statistics: TransportStatistics,
    start_error: Option<TransportError>,
    send_error: Option<TransportError>,
    close_error: Option<TransportError>,
}

impl MemoryTransport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_incoming(
        incoming: impl IntoIterator<Item = Result<ReceivedFrame, TransportError>>,
    ) -> Self {
        Self {
            incoming: incoming.into_iter().collect(),
            ..Self::default()
        }
    }

    pub fn fail_start_with(&mut self, error: TransportError) {
        self.start_error = Some(error);
    }

    pub fn fail_send_with(&mut self, error: TransportError) {
        self.send_error = Some(error);
    }

    pub fn fail_close_with(&mut self, error: TransportError) {
        self.close_error = Some(error);
    }

    pub fn push_frame(&mut self, frame: ReceivedFrame) {
        self.incoming.push_back(Ok(frame));
    }

    pub fn push_error(&mut self, error: TransportError) {
        self.incoming.push_back(Err(error));
    }

    pub fn sent_payloads(&self) -> &[Vec<u8>] {
        &self.sent
    }

    pub fn events(&self) -> &[MemoryTransportEvent] {
        &self.events
    }
}

impl Transport for MemoryTransport {
    fn start_receive(&mut self) -> Result<(), TransportError> {
        if self.closed {
            return Err(TransportError::Closed);
        }
        if let Some(error) = self.start_error.take() {
            return Err(error);
        }
        self.receive_ready = true;
        self.statistics.receive_ready = true;
        self.events.push(MemoryTransportEvent::ReceiveStarted);
        Ok(())
    }

    fn send(&mut self, payload: &[u8]) -> Result<(), TransportError> {
        if self.closed {
            return Err(TransportError::Closed);
        }
        if !self.receive_ready {
            return Err(TransportError::NotReady);
        }
        if let Some(error) = self.send_error.take() {
            return Err(error);
        }
        self.sent.push(payload.to_vec());
        self.statistics.sent_frames += 1;
        self.events.push(MemoryTransportEvent::Sent);
        Ok(())
    }

    fn receive(&mut self, deadline: Instant) -> Result<ReceivedFrame, TransportError> {
        if self.closed {
            return Err(TransportError::Closed);
        }
        if !self.receive_ready {
            return Err(TransportError::NotReady);
        }
        let result = if deadline <= Instant::now() {
            Err(TransportError::TimedOut)
        } else {
            self.incoming
                .pop_front()
                .unwrap_or(Err(TransportError::TimedOut))
        };
        match &result {
            Ok(_) => {
                self.statistics.received_frames += 1;
                self.events.push(MemoryTransportEvent::Received);
            }
            Err(TransportError::TimedOut) => {
                self.statistics.receive_timeouts += 1;
                self.events.push(MemoryTransportEvent::TimedOut);
            }
            Err(_) => {}
        }
        result
    }

    fn statistics(&self) -> TransportStatistics {
        self.statistics
    }

    fn close(&mut self) -> Result<(), TransportError> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        self.receive_ready = false;
        self.statistics.receive_ready = false;
        self.events.push(MemoryTransportEvent::Closed);
        self.close_error.take().map_or(Ok(()), Err)
    }
}
