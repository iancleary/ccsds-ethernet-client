use std::error::Error;
use std::fmt;
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use ccsds_ethernet_client::{
    Codec, DecodeResult, DecodedMessage, EncodedCommand, Endpoint, MemoryTransport, ReceivedFrame,
    Session, TransportError,
};

// These APIDs and application-data values are local to this example. A mission
// codec must define its own packet schema and allocation policy.
const COMMAND_APID: u16 = 0x120;
const TELEMETRY_APID: u16 = 0x121;
const ACKNOWLEDGEMENT_APID: u16 = 0x122;
const INCREMENT_COMMAND: u8 = 0x01;
const COUNTER_TELEMETRY: u8 = 0x02;
const COMMAND_ACCEPTED: u8 = 0x03;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PacketType {
    Telemetry,
    Telecommand,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DummyCommand {
    command_counter: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DummyAcknowledgement {
    command_counter: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DummyTelemetry {
    command_counter: u16,
}

#[derive(Debug)]
struct DummyCodecError(String);

impl DummyCodecError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for DummyCodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for DummyCodecError {}

struct SpacePacket<'a> {
    packet_type: PacketType,
    apid: u16,
    data: &'a [u8],
}

impl SpacePacket<'_> {
    fn encode(
        packet_type: PacketType,
        apid: u16,
        sequence_count: u16,
        data: &[u8],
    ) -> Result<Vec<u8>, DummyCodecError> {
        if apid > 0x07ff {
            return Err(DummyCodecError::new("APID exceeds 11 bits"));
        }
        if sequence_count > 0x3fff {
            return Err(DummyCodecError::new("sequence count exceeds 14 bits"));
        }
        if data.is_empty() || data.len() > 65_536 {
            return Err(DummyCodecError::new(
                "packet data field must contain 1 through 65536 octets",
            ));
        }

        let packet_type_bit = match packet_type {
            PacketType::Telemetry => 0,
            PacketType::Telecommand => 1 << 12,
        };
        let packet_id = packet_type_bit | apid;
        let sequence_control = (0b11 << 14) | sequence_count;
        let packet_data_length = u16::try_from(data.len() - 1)
            .map_err(|_| DummyCodecError::new("packet data field is too long"))?;

        let mut packet = Vec::with_capacity(6 + data.len());
        packet.extend_from_slice(&packet_id.to_be_bytes());
        packet.extend_from_slice(&sequence_control.to_be_bytes());
        packet.extend_from_slice(&packet_data_length.to_be_bytes());
        packet.extend_from_slice(data);
        Ok(packet)
    }

    fn decode(payload: &[u8]) -> Result<SpacePacket<'_>, DummyCodecError> {
        if payload.len() < 7 {
            return Err(DummyCodecError::new(
                "space packet is shorter than 7 octets",
            ));
        }

        let packet_id = u16::from_be_bytes([payload[0], payload[1]]);
        if packet_id >> 13 != 0 {
            return Err(DummyCodecError::new("unsupported CCSDS packet version"));
        }
        if packet_id & (1 << 11) != 0 {
            return Err(DummyCodecError::new(
                "this example does not define a secondary header",
            ));
        }

        let sequence_control = u16::from_be_bytes([payload[2], payload[3]]);
        if sequence_control >> 14 != 0b11 {
            return Err(DummyCodecError::new(
                "this example accepts unsegmented packets only",
            ));
        }

        let data_length = usize::from(u16::from_be_bytes([payload[4], payload[5]])) + 1;
        if payload.len() != 6 + data_length {
            return Err(DummyCodecError::new(
                "packet data length does not match the payload",
            ));
        }

        Ok(SpacePacket {
            packet_type: if packet_id & (1 << 12) == 0 {
                PacketType::Telemetry
            } else {
                PacketType::Telecommand
            },
            apid: packet_id & 0x07ff,
            data: &payload[6..],
        })
    }
}

struct DummyCodec;

impl Codec for DummyCodec {
    type Command = DummyCommand;
    type Acknowledgement = DummyAcknowledgement;
    type Telemetry = DummyTelemetry;
    type Correlation = u16;
    type Error = DummyCodecError;

    fn encode_command(
        &mut self,
        command: &Self::Command,
    ) -> Result<EncodedCommand<Self::Correlation>, Self::Error> {
        let counter = command.command_counter.to_be_bytes();
        let data = [INCREMENT_COMMAND, counter[0], counter[1]];
        let payload = SpacePacket::encode(
            PacketType::Telecommand,
            COMMAND_APID,
            command.command_counter,
            &data,
        )?;
        Ok(EncodedCommand::new(payload, command.command_counter))
    }

    fn decode(
        &mut self,
        payload: &[u8],
        _sender: Endpoint,
    ) -> DecodeResult<Self::Acknowledgement, Self::Telemetry, Self::Correlation, Self::Error> {
        let packet = SpacePacket::decode(payload)?;
        match (packet.packet_type, packet.apid, packet.data) {
            (PacketType::Telemetry, TELEMETRY_APID, [COUNTER_TELEMETRY, high, low]) => {
                Ok(DecodedMessage::Telemetry(DummyTelemetry {
                    command_counter: u16::from_be_bytes([*high, *low]),
                }))
            }
            (PacketType::Telemetry, ACKNOWLEDGEMENT_APID, [COMMAND_ACCEPTED, high, low]) => {
                let command_counter = u16::from_be_bytes([*high, *low]);
                Ok(DecodedMessage::Acknowledgement {
                    acknowledgement: DummyAcknowledgement { command_counter },
                    correlation: command_counter,
                })
            }
            _ => Ok(DecodedMessage::Ignored),
        }
    }
}

#[derive(Debug)]
struct DemoResult {
    command_packet: Vec<u8>,
    telemetry_packet: Vec<u8>,
    acknowledgement_packet: Vec<u8>,
    acknowledgement: DummyAcknowledgement,
    telemetry: DummyTelemetry,
}

fn board_endpoint() -> Result<Endpoint, Box<dyn Error>> {
    Ok(Endpoint::new(Ipv4Addr::new(169, 254, 209, 0), 24_576)?)
}

fn incoming_frame(payload: Vec<u8>, sender: Endpoint) -> Result<ReceivedFrame, TransportError> {
    Ok(ReceivedFrame { payload, sender })
}

fn run_demo() -> Result<DemoResult, Box<dyn Error>> {
    let command = DummyCommand {
        command_counter: 41,
    };
    let incremented_counter = command
        .command_counter
        .checked_add(1)
        .ok_or_else(|| DummyCodecError::new("command counter overflow"))?;
    let telemetry_counter = incremented_counter.to_be_bytes();
    let command_counter = command.command_counter.to_be_bytes();

    // The telemetry and acknowledgement are dummy board responses. Their
    // primary-header sequence counts are independent of the application-level
    // command counter carried in their data fields.
    let telemetry_packet = SpacePacket::encode(
        PacketType::Telemetry,
        TELEMETRY_APID,
        0,
        &[
            COUNTER_TELEMETRY,
            telemetry_counter[0],
            telemetry_counter[1],
        ],
    )?;
    let acknowledgement_packet = SpacePacket::encode(
        PacketType::Telemetry,
        ACKNOWLEDGEMENT_APID,
        0,
        &[COMMAND_ACCEPTED, command_counter[0], command_counter[1]],
    )?;

    let board = board_endpoint()?;
    let transport = MemoryTransport::with_incoming([
        incoming_frame(telemetry_packet.clone(), board),
        incoming_frame(acknowledgement_packet.clone(), board),
    ]);
    let mut session = Session::from_transport(DummyCodec, transport, board)?;
    let deadline = Instant::now() + Duration::from_secs(1);
    let acknowledgement = session.exchange_once(&command, deadline)?;
    let telemetry = session.next_telemetry(deadline)?;
    let command_packet = session.into_transport().sent_payloads()[0].clone();

    Ok(DemoResult {
        command_packet,
        telemetry_packet,
        acknowledgement_packet,
        acknowledgement,
        telemetry,
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn main() -> Result<(), Box<dyn Error>> {
    let result = run_demo()?;
    println!("command packet:   {}", hex(&result.command_packet));
    println!("telemetry packet: {}", hex(&result.telemetry_packet));
    println!("ack packet:       {}", hex(&result.acknowledgement_packet));
    println!(
        "acknowledged command counter: {}",
        result.acknowledgement.command_counter
    );
    println!(
        "telemetry command counter:    {}",
        result.telemetry.command_counter
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_has_expected_ccsds_primary_header_and_dummy_data() {
        let result = run_demo().expect("demo exchange");
        assert_eq!(
            result.command_packet,
            [0x11, 0x20, 0xc0, 0x29, 0x00, 0x02, 0x01, 0x00, 0x29]
        );
    }

    #[test]
    fn telemetry_returns_the_incremented_command_counter() {
        let result = run_demo().expect("demo exchange");
        assert_eq!(
            result.telemetry_packet,
            [0x01, 0x21, 0xc0, 0x00, 0x00, 0x02, 0x02, 0x00, 0x2a]
        );
        assert_eq!(
            result.acknowledgement_packet,
            [0x01, 0x22, 0xc0, 0x00, 0x00, 0x02, 0x03, 0x00, 0x29]
        );
        assert_eq!(result.acknowledgement.command_counter, 41);
        assert_eq!(result.telemetry.command_counter, 42);
    }
}
