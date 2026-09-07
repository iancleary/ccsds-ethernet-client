import sys
import unittest

from ccsds_ethernet_client import (
    ConfigError,
    FrameError,
    RawEthernetClient,
    RawEthernetConfig,
    build_udp_frame,
    parse_udp_frame,
)


def ipv4_config() -> RawEthernetConfig:
    return RawEthernetConfig(
        interface_name="eth0",
        host_mac="02:00:00:00:00:01",
        host_ip="169.254.209.1",
        host_udp_port=49152,
        board_mac="02:00:00:00:00:7a",
        board_ip="169.254.209.0",
        board_udp_port=24576,
        ring_capacity=8,
    )


def reverse(config: RawEthernetConfig) -> RawEthernetConfig:
    return RawEthernetConfig(
        interface_name=config.interface_name,
        host_mac=config.board_mac,
        host_ip=config.board_ip,
        host_udp_port=config.board_udp_port,
        board_mac=config.host_mac,
        board_ip=config.host_ip,
        board_udp_port=config.host_udp_port,
        ring_capacity=config.ring_capacity,
    )


class BindingContractTests(unittest.TestCase):
    def test_config_exposes_strict_rust_validation(self) -> None:
        config = ipv4_config()
        self.assertEqual(config.schema_version, 2)
        self.assertEqual(config.maximum_udp_payload_bytes, 1472)
        self.assertEqual(config.board_ip, "169.254.209.0")

        with self.assertRaises(ConfigError):
            RawEthernetConfig(
                interface_name="../eth0",
                host_mac=config.host_mac,
                host_ip=config.host_ip,
                host_udp_port=config.host_udp_port,
                board_mac=config.board_mac,
                board_ip=config.board_ip,
                board_udp_port=config.board_udp_port,
                ring_capacity=config.ring_capacity,
            )

    def test_frame_helpers_round_trip_bytes_without_hardware(self) -> None:
        config = ipv4_config()
        frame = build_udp_frame(reverse(config), b"\x01\x02\x03", ipv4_identification=17)
        datagram = parse_udp_frame(config, frame)

        self.assertIsNotNone(datagram)
        assert datagram is not None
        self.assertEqual(datagram.payload, b"\x01\x02\x03")
        self.assertEqual(datagram.sender_ip, config.board_ip)
        self.assertEqual(datagram.sender_udp_port, config.board_udp_port)

    def test_foreign_frames_return_none_and_invalid_frames_raise(self) -> None:
        config = ipv4_config()
        frame = bytearray(build_udp_frame(reverse(config), b"payload"))
        frame[0] ^= 0x10
        self.assertIsNone(parse_udp_frame(config, bytes(frame)))

        with self.assertRaises(FrameError):
            parse_udp_frame(config, b"short")

    @unittest.skipIf(sys.platform.startswith("linux"), "non-Linux contract")
    def test_live_client_is_explicitly_linux_only(self) -> None:
        with self.assertRaises(NotImplementedError):
            RawEthernetClient(ipv4_config())


if __name__ == "__main__":
    unittest.main()
