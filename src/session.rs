use std::collections::VecDeque;
use std::error::Error;
use std::fmt;
use std::time::Instant;

#[rustfmt::skip]
use crate::{
    Endpoint,
    ReceivedFrame,
    Transport,
    TransportError,
    TransportStatistics,
};

pub type DecodeResult<A, T, C, E> = Result<DecodedMessage<A, T, C>, E>;

pub trait Codec {
    type Command;
    type Acknowledgement;
    type Telemetry;
    /// Consumer-owned request identity. Equality is not proof of freshness:
    /// do not reuse an identity while a stale acknowledgement can arrive.
    type Correlation: Eq;
    type Error: Error + Send + Sync + 'static;

    fn encode_command(
        &mut self,
        command: &Self::Command,
    ) -> Result<EncodedCommand<Self::Correlation>, Self::Error>;

    fn decode(
        &mut self,
        payload: &[u8],
        sender: Endpoint,
    ) -> DecodeResult<Self::Acknowledgement, Self::Telemetry, Self::Correlation, Self::Error>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncodedCommand<C> {
    payload: Vec<u8>,
    correlation: C,
}

impl<C> EncodedCommand<C> {
    pub fn new(payload: Vec<u8>, correlation: C) -> Self {
        Self {
            payload,
            correlation,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecodedMessage<A, T, C> {
    Acknowledgement { acknowledgement: A, correlation: C },
    Telemetry(T),
    Ignored,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionStatistics {
    diagnostics: SessionDiagnostics,
    transport: TransportStatistics,
    queued_telemetry: usize,
    telemetry_queue_capacity: usize,
    dropped_telemetry: u64,
}

/// Lifetime observations, including events seen during failed exchanges.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SessionDiagnostics {
    pub decode_failures: u64,
    pub unmatched_acknowledgements: u64,
    pub ignored_messages: u64,
    pub foreign_senders: u64,
}

impl SessionStatistics {
    pub const fn diagnostics(self) -> SessionDiagnostics {
        self.diagnostics
    }
    pub const fn transport(self) -> TransportStatistics {
        self.transport
    }

    pub const fn queued_telemetry(self) -> usize {
        self.queued_telemetry
    }

    pub const fn telemetry_queue_capacity(self) -> usize {
        self.telemetry_queue_capacity
    }

    pub const fn dropped_telemetry(self) -> u64 {
        self.dropped_telemetry
    }
}

#[derive(Debug)]
pub enum ExchangeError<E> {
    Closed,
    DeadlineExpiredBeforeSend,
    Encode(E),
    Send(TransportError),
    DeliveryOutcomeUnknown { source: TransportError },
}

impl<E> ExchangeError<E> {
    pub const fn delivery_outcome_unknown(&self) -> bool {
        matches!(self, Self::DeliveryOutcomeUnknown { .. })
    }
}

impl<E: fmt::Display> fmt::Display for ExchangeError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => formatter.write_str("session is closed"),
            Self::DeadlineExpiredBeforeSend => {
                formatter.write_str("exchange deadline expired before send")
            }
            Self::Encode(error) => write!(formatter, "command encoding failed: {error}"),
            Self::Send(error) => write!(formatter, "command send failed: {error}"),
            Self::DeliveryOutcomeUnknown { source } => {
                write!(formatter, "delivery outcome unknown: {source}")
            }
        }
    }
}

impl<E: Error + 'static> Error for ExchangeError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Encode(error) => Some(error),
            Self::Send(error) | Self::DeliveryOutcomeUnknown { source: error } => Some(error),
            Self::Closed | Self::DeadlineExpiredBeforeSend => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReceiveError {
    Closed,
    DeadlineExpired,
    Transport(TransportError),
}

impl fmt::Display for ReceiveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => formatter.write_str("session is closed"),
            Self::DeadlineExpired => formatter.write_str("receive deadline expired"),
            Self::Transport(error) => write!(formatter, "telemetry receive failed: {error}"),
        }
    }
}

impl Error for ReceiveError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Transport(error) => Some(error),
            Self::Closed | Self::DeadlineExpired => None,
        }
    }
}

pub struct Session<C: Codec, T: Transport> {
    diagnostics: SessionDiagnostics,
    codec: C,
    transport: T,
    remote: Endpoint,
    telemetry: VecDeque<C::Telemetry>,
    telemetry_capacity: usize,
    dropped_telemetry: u64,
    closed: bool,
}

impl<C: Codec, T: Transport> Session<C, T> {
    pub fn from_transport(
        codec: C,
        mut transport: T,
        remote: Endpoint,
    ) -> Result<Self, TransportError> {
        transport.start_receive()?;
        let telemetry_capacity = transport.statistics().queue_capacity.max(1);
        let mut telemetry = VecDeque::new();
        telemetry
            .try_reserve_exact(telemetry_capacity)
            .map_err(|error| {
                TransportError::Other(format!("cannot allocate telemetry queue: {error}"))
            })?;
        Ok(Self {
            diagnostics: SessionDiagnostics::default(),
            codec,
            transport,
            remote,
            telemetry,
            telemetry_capacity,
            dropped_telemetry: 0,
            closed: false,
        })
    }

    pub fn exchange_once(
        &mut self,
        command: &C::Command,
        deadline: Instant,
    ) -> Result<C::Acknowledgement, ExchangeError<C::Error>> {
        if self.closed {
            return Err(ExchangeError::Closed);
        }
        if Instant::now() >= deadline {
            return Err(ExchangeError::DeadlineExpiredBeforeSend);
        }
        let encoded = self
            .codec
            .encode_command(command)
            .map_err(ExchangeError::Encode)?;
        if Instant::now() >= deadline {
            return Err(ExchangeError::DeadlineExpiredBeforeSend);
        }
        self.transport
            .send(&encoded.payload)
            .map_err(ExchangeError::Send)?;

        loop {
            let frame = self
                .transport
                .receive(deadline)
                .map_err(|source| ExchangeError::DeliveryOutcomeUnknown { source })?;
            if frame.sender != self.remote {
                self.diagnostics.foreign_senders =
                    self.diagnostics.foreign_senders.saturating_add(1);
                continue;
            }
            match self.codec.decode(&frame.payload, frame.sender) {
                Ok(DecodedMessage::Acknowledgement {
                    acknowledgement,
                    correlation,
                }) if correlation == encoded.correlation => return Ok(acknowledgement),
                Ok(DecodedMessage::Telemetry(telemetry)) => self.queue_telemetry(telemetry),
                Ok(DecodedMessage::Acknowledgement { .. }) => self.record_unmatched(),
                Ok(DecodedMessage::Ignored) => self.record_ignored(),
                Err(_) => self.record_decode_failure(),
            }
        }
    }

    pub fn next_telemetry(&mut self, deadline: Instant) -> Result<C::Telemetry, ReceiveError> {
        if self.closed {
            return Err(ReceiveError::Closed);
        }
        if let Some(telemetry) = self.telemetry.pop_front() {
            return Ok(telemetry);
        }
        loop {
            let ReceivedFrame { payload, sender } = match self.transport.receive(deadline) {
                Ok(frame) => frame,
                Err(TransportError::TimedOut) => return Err(ReceiveError::DeadlineExpired),
                Err(error) => return Err(ReceiveError::Transport(error)),
            };
            if sender != self.remote {
                self.diagnostics.foreign_senders =
                    self.diagnostics.foreign_senders.saturating_add(1);
                continue;
            }
            match self.codec.decode(&payload, sender) {
                Ok(DecodedMessage::Telemetry(telemetry)) => return Ok(telemetry),
                Ok(DecodedMessage::Acknowledgement { .. }) => self.record_unmatched(),
                Ok(DecodedMessage::Ignored) => self.record_ignored(),
                Err(_) => self.record_decode_failure(),
            }
        }
    }

    pub fn statistics(&self) -> SessionStatistics {
        SessionStatistics {
            diagnostics: self.diagnostics,
            transport: self.transport.statistics(),
            queued_telemetry: self.telemetry.len(),
            telemetry_queue_capacity: self.telemetry_capacity,
            dropped_telemetry: self.dropped_telemetry,
        }
    }

    pub fn close(&mut self) -> Result<(), TransportError> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        self.transport.close()
    }

    pub const fn is_closed(&self) -> bool {
        self.closed
    }

    pub fn into_transport(self) -> T {
        self.transport
    }

    fn queue_telemetry(&mut self, telemetry: C::Telemetry) {
        if self.telemetry.len() == self.telemetry_capacity {
            self.dropped_telemetry = self.dropped_telemetry.saturating_add(1);
        } else {
            self.telemetry.push_back(telemetry);
        }
    }

    fn record_unmatched(&mut self) {
        self.diagnostics.unmatched_acknowledgements = self
            .diagnostics
            .unmatched_acknowledgements
            .saturating_add(1);
    }
    fn record_ignored(&mut self) {
        self.diagnostics.ignored_messages = self.diagnostics.ignored_messages.saturating_add(1);
    }
    fn record_decode_failure(&mut self) {
        self.diagnostics.decode_failures = self.diagnostics.decode_failures.saturating_add(1);
    }
}

#[cfg(target_os = "linux")]
impl<C: Codec> Session<C, crate::LinuxRawEthernetTransport> {
    pub fn open(config: crate::RawEthernetConfig, codec: C) -> Result<Self, TransportError> {
        let remote = config.board().network();
        let transport = crate::LinuxRawEthernetTransport::open(config)?;
        Self::from_transport(codec, transport, remote)
    }
}
