use std::error::Error as _;
use std::fmt;
use std::net::Ipv4Addr;
#[rustfmt::skip]
use std::time::{
    Duration,
    Instant,
};

use ccsds_ethernet_client::{
    Codec, DecodeResult, DecodedMessage, EncodedCommand, Endpoint, ExchangeError, MemoryTransport,
    MemoryTransportEvent, ReceiveError, ReceivedFrame, Session, TransportError,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Command(u8);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Acknowledgement {
    request: u8,
    status: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Telemetry(u8);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Correlation(u8);

#[derive(Debug)]
struct CodecError;

impl fmt::Display for CodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid test payload")
    }
}

impl std::error::Error for CodecError {}

struct TestCodec;

impl Codec for TestCodec {
    type Command = Command;
    type Acknowledgement = Acknowledgement;
    type Telemetry = Telemetry;
    type Correlation = Correlation;
    type Error = CodecError;

    fn encode_command(
        &mut self,
        command: &Self::Command,
    ) -> Result<EncodedCommand<Self::Correlation>, Self::Error> {
        if command.0 == 0xff {
            return Err(CodecError);
        }
        Ok(EncodedCommand::new(
            vec![0x10, command.0],
            Correlation(command.0),
        ))
    }

    fn decode(
        &mut self,
        payload: &[u8],
        _sender: Endpoint,
    ) -> DecodeResult<Self::Acknowledgement, Self::Telemetry, Self::Correlation, Self::Error> {
        match payload {
            [0x20, request, status] => Ok(DecodedMessage::Acknowledgement {
                acknowledgement: Acknowledgement {
                    request: *request,
                    status: *status,
                },
                correlation: Correlation(*request),
            }),
            [0x30, value] => Ok(DecodedMessage::Telemetry(Telemetry(*value))),
            [0xff] => Ok(DecodedMessage::Ignored),
            _ => Err(CodecError),
        }
    }
}

fn remote() -> Endpoint {
    Endpoint::new("169.254.209.0".parse::<Ipv4Addr>().expect("IP"), 24_576).expect("endpoint")
}

fn frame(payload: &[u8]) -> Result<ReceivedFrame, TransportError> {
    Ok(ReceivedFrame {
        payload: payload.to_vec(),
        sender: remote(),
    })
}

fn future_deadline() -> Instant {
    Instant::now() + Duration::from_secs(1)
}

#[test]
fn reused_correlation_cannot_distinguish_a_stale_acknowledgement() {
    // This documents the codec boundary, not a transport freshness guarantee.
    // Consumers must choose distinct identities while old responses can exist.
    let transport = MemoryTransport::with_incoming([frame(&[0x20, 1, 0]), frame(&[0x20, 1, 9])]);
    let mut session = Session::from_transport(TestCodec, transport, remote()).unwrap();
    assert_eq!(
        session
            .exchange_once(&Command(1), future_deadline())
            .unwrap()
            .status,
        0
    );
    assert_eq!(
        session
            .exchange_once(&Command(1), future_deadline())
            .unwrap()
            .status,
        9
    );
}

#[test]
fn diagnostics_survive_unknown_delivery_and_telemetry_receive() {
    let transport =
        MemoryTransport::with_incoming([frame(&[0x20]), frame(&[0x20, 8, 0]), frame(&[0xff])]);
    let mut session = Session::from_transport(TestCodec, transport, remote()).unwrap();
    assert!(
        session
            .exchange_once(&Command(9), future_deadline())
            .unwrap_err()
            .delivery_outcome_unknown()
    );
    let diagnostics = session.statistics().diagnostics();
    assert_eq!(diagnostics.decode_failures, 1);
    assert_eq!(diagnostics.unmatched_acknowledgements, 1);
    assert_eq!(diagnostics.ignored_messages, 1);
    assert_eq!(session.into_transport().sent_payloads().len(), 1);

    let transport = MemoryTransport::with_incoming([
        frame(&[0x20]),
        frame(&[0x20, 8, 0]),
        frame(&[0xff]),
        frame(&[0x30, 7]),
    ]);
    let mut session = Session::from_transport(TestCodec, transport, remote()).unwrap();
    assert_eq!(session.next_telemetry(future_deadline()), Ok(Telemetry(7)));
    assert_eq!(session.statistics().diagnostics(), diagnostics);
}

#[test]
fn distinct_request_identity_rejects_delayed_duplicate_ack() {
    let transport = MemoryTransport::with_incoming([
        frame(&[0x20, 8, 0]),
        frame(&[0x20, 8, 0]),
        frame(&[0x20, 9, 0]),
    ]);
    let mut session = Session::from_transport(TestCodec, transport, remote()).unwrap();
    assert_eq!(
        session
            .exchange_once(&Command(8), future_deadline())
            .unwrap()
            .request,
        8
    );
    assert_eq!(
        session
            .exchange_once(&Command(9), future_deadline())
            .unwrap()
            .request,
        9
    );
    assert_eq!(
        session
            .statistics()
            .diagnostics()
            .unmatched_acknowledgements,
        1
    );
}

#[test]
fn os_error_preserves_machine_readable_evidence() {
    let error = TransportError::from_io("send", std::io::Error::from_raw_os_error(13));
    assert!(matches!(
        error,
        TransportError::Io {
            operation: "send",
            raw_os_error: Some(13),
            ..
        }
    ));
}

#[test]
fn exchange_starts_receive_first_correlates_and_preserves_typed_results() {
    let transport = MemoryTransport::with_incoming([
        frame(&[0x30, 7]),
        frame(&[0x20, 8, 0]),
        frame(&[0x20, 9, 0x81]),
    ]);
    let mut session = Session::from_transport(TestCodec, transport, remote()).expect("session");

    let acknowledgement = session
        .exchange_once(&Command(9), future_deadline())
        .expect("matching acknowledgement");
    assert_eq!(
        acknowledgement,
        Acknowledgement {
            request: 9,
            status: 0x81,
        }
    );
    assert_eq!(session.next_telemetry(future_deadline()), Ok(Telemetry(7)));

    let transport = session.into_transport();
    assert_eq!(transport.sent_payloads(), &[vec![0x10, 9]]);
    assert_eq!(
        transport.events(),
        &[
            MemoryTransportEvent::ReceiveStarted,
            MemoryTransportEvent::Sent,
            MemoryTransportEvent::Received,
            MemoryTransportEvent::Received,
            MemoryTransportEvent::Received,
        ]
    );
}

#[test]
fn expired_before_send_and_post_send_timeout_are_distinct_and_never_retry() {
    let transport = MemoryTransport::new();
    let mut session = Session::from_transport(TestCodec, transport, remote()).expect("session");
    let error = session
        .exchange_once(&Command(1), Instant::now())
        .expect_err("deadline already expired");
    assert!(matches!(error, ExchangeError::DeadlineExpiredBeforeSend));
    assert!(!error.delivery_outcome_unknown());
    assert!(error.source().is_none());
    let transport = session.into_transport();
    assert!(transport.sent_payloads().is_empty());

    let transport = MemoryTransport::new();
    let mut session = Session::from_transport(TestCodec, transport, remote()).expect("session");
    let error = session
        .exchange_once(&Command(2), future_deadline())
        .expect_err("missing acknowledgement");
    assert!(matches!(
        error,
        ExchangeError::DeliveryOutcomeUnknown {
            source: TransportError::TimedOut
        }
    ));
    assert!(error.delivery_outcome_unknown());
    assert!(
        error
            .source()
            .is_some_and(|source| source.is::<TransportError>())
    );
    let transport = session.into_transport();
    assert_eq!(transport.sent_payloads(), &[vec![0x10, 2]]);
    assert_eq!(
        transport.events(),
        &[
            MemoryTransportEvent::ReceiveStarted,
            MemoryTransportEvent::Sent,
            MemoryTransportEvent::TimedOut,
        ]
    );
}

#[test]
fn telemetry_queue_is_bounded_and_reports_drops() {
    let transport = MemoryTransport::with_incoming([
        frame(&[0x30, 1]),
        frame(&[0x30, 2]),
        frame(&[0x20, 3, 0]),
    ]);
    let mut session = Session::from_transport(TestCodec, transport, remote()).expect("session");
    session
        .exchange_once(&Command(3), future_deadline())
        .expect("acknowledgement");

    let statistics = session.statistics();
    assert_eq!(statistics.telemetry_queue_capacity(), 1);
    assert_eq!(statistics.queued_telemetry(), 1);
    assert_eq!(statistics.dropped_telemetry(), 1);
    assert_eq!(session.next_telemetry(future_deadline()), Ok(Telemetry(1)));
}

#[test]
fn close_is_idempotent_and_blocks_further_work() {
    let transport = MemoryTransport::new();
    let mut session = Session::from_transport(TestCodec, transport, remote()).expect("session");
    session.close().expect("first close");
    session.close().expect("second close");
    assert!(session.is_closed());
    assert!(matches!(
        session.exchange_once(&Command(4), future_deadline()),
        Err(ExchangeError::Closed)
    ));
    assert_eq!(
        session.next_telemetry(future_deadline()),
        Err(ReceiveError::Closed)
    );

    let transport = session.into_transport();
    assert_eq!(
        transport.events(),
        &[
            MemoryTransportEvent::ReceiveStarted,
            MemoryTransportEvent::Closed,
        ]
    );
}

#[test]
fn close_error_still_transitions_session_to_closed_once() {
    let mut transport = MemoryTransport::new();
    transport.fail_close_with(TransportError::Other("close failed".to_owned()));
    let mut session = Session::from_transport(TestCodec, transport, remote()).expect("session");

    assert_eq!(
        session.close(),
        Err(TransportError::Other("close failed".to_owned()))
    );
    assert!(session.is_closed());
    session.close().expect("second close is idempotent");
    assert!(matches!(
        session.exchange_once(&Command(4), future_deadline()),
        Err(ExchangeError::Closed)
    ));
    assert_eq!(
        session.next_telemetry(future_deadline()),
        Err(ReceiveError::Closed)
    );

    let transport = session.into_transport();
    assert_eq!(
        transport.events(),
        &[
            MemoryTransportEvent::ReceiveStarted,
            MemoryTransportEvent::Closed,
        ]
    );
}

#[test]
fn wrapped_errors_expose_sources_and_state_errors_do_not() {
    let transport = MemoryTransport::new();
    let mut session = Session::from_transport(TestCodec, transport, remote()).expect("session");
    let encode = session
        .exchange_once(&Command(0xff), future_deadline())
        .expect_err("encode failure");
    assert!(
        encode
            .source()
            .is_some_and(|source| source.is::<CodecError>())
    );

    let mut transport = MemoryTransport::new();
    transport.fail_send_with(TransportError::Other("send failed".to_owned()));
    let mut session = Session::from_transport(TestCodec, transport, remote()).expect("session");
    let send = session
        .exchange_once(&Command(1), future_deadline())
        .expect_err("send failure");
    assert!(
        send.source()
            .is_some_and(|source| source.is::<TransportError>())
    );

    let mut transport = MemoryTransport::new();
    transport.push_error(TransportError::Other("receive failed".to_owned()));
    let mut session = Session::from_transport(TestCodec, transport, remote()).expect("session");
    let receive = session
        .next_telemetry(future_deadline())
        .expect_err("receive failure");
    assert!(
        receive
            .source()
            .is_some_and(|source| source.is::<TransportError>())
    );

    let timeout = session
        .next_telemetry(Instant::now())
        .expect_err("deadline failure");
    assert_eq!(timeout, ReceiveError::DeadlineExpired);
    assert!(timeout.source().is_none());
    session.close().expect("close");
    let closed = session
        .next_telemetry(future_deadline())
        .expect_err("closed session");
    assert_eq!(closed, ReceiveError::Closed);
    assert!(closed.source().is_none());
}

#[test]
fn lifecycle_start_failure_is_returned() {
    let mut transport = MemoryTransport::new();
    transport.fail_start_with(TransportError::Other("listener failed".to_owned()));
    let result = Session::from_transport(TestCodec, transport, remote());
    assert!(matches!(result, Err(TransportError::Other(message)) if message == "listener failed"));
}
