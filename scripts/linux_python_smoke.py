"""Installed-wheel smoke test; run inside the disposable veth fixture only."""
import os
import socket
import subprocess
from concurrent.futures import ThreadPoolExecutor
from threading import Event
from unittest.mock import patch

from ccsds_ethernet_client import RawEthernetClient, RawEthernetConfig, ThreadedRawEthernetClient, TransportError, build_udp_frame

assert os.environ.get("CCSDS_LIVE_TEST") == "1", "isolated fixture required"

def link(name, *args):
    assert name in {"ccsds-host", "ccsds-peer"}
    subprocess.run(["ip", "link", "set", name, *args], check=True)

for name in ("ccsds-host", "ccsds-peer"):
    link(name, "mtu", "4096")

def config(reverse=False):
    return RawEthernetConfig(
        interface_name="ccsds-peer" if reverse else "ccsds-host",
        host_mac="02:00:00:00:00:02" if reverse else "02:00:00:00:00:01",
        host_ip="192.0.2.2" if reverse else "192.0.2.1",
        host_udp_port=40002 if reverse else 40001,
        board_mac="02:00:00:00:00:01" if reverse else "02:00:00:00:00:02",
        board_ip="192.0.2.1" if reverse else "192.0.2.2",
        board_udp_port=40001 if reverse else 40002,
        ring_capacity=1,
        ipv4_checksum_policy="require",
    )

with ThreadedRawEthernetClient(config()) as host, ThreadedRawEthernetClient(config(True)) as peer:
    waiting = Event()
    original_wait = host._condition.wait
    def wait(timeout):
        waiting.set()
        return original_wait(timeout)
    with patch.object(host._condition, "wait", wait), ThreadPoolExecutor(1) as pool:
        cancelled = pool.submit(host.receive, 60)
        assert waiting.wait(2)
        host.cancel_receive()
        try:
            cancelled.result(2)
        except TransportError as error:
            assert "cancelled" in str(error)
        else:
            raise AssertionError("receive cancellation did not wake the waiter")
    with ThreadPoolExecutor(1) as pool:
        waiting = pool.submit(host.receive, 2)
        host.send(b"command")
        assert peer.receive(2).payload == b"command"
        peer.send(b"ack")
        assert waiting.result(2).payload == b"ack"

    with socket.socket(socket.AF_PACKET, socket.SOCK_RAW) as injector:
        injector.bind(("ccsds-peer", 0))
        valid = build_udp_frame(config(True), b"injected")
        bad = bytearray(valid)
        bad[42] ^= 1
        injector.send(bad)
        tagged = valid[:12] + b"\x81\x00\x00\x01" + valid[12:]
        injector.send(tagged)
        injector.send(valid + bytes(2200 - len(valid)))
        peer.send(b"after-invalid")
        assert host.receive(2).payload == b"after-invalid"
    # Receipt ordering proves the owner processed earlier injected frames.
    assert host.statistics().invalid_frames >= 1
    assert host.statistics().vlan_frames >= 1
    assert host.statistics().truncated_frames >= 1

# Down is not peer death, and send failure must retain OS evidence. Recovery
# requires an explicit reopen; no command is retried by either facade.
with RawEthernetClient(config(True)) as peer:
    link("ccsds-peer", "down")
    try:
        try:
            peer.send(b"must-fail")
        except TransportError as error:
            assert error.operation == "sending AF_PACKET frame"
            assert error.errno is not None
            assert error.kind is not None
        else:
            raise AssertionError("send on down interface unexpectedly succeeded")
    finally:
        link("ccsds-peer", "up")
with ThreadedRawEthernetClient(config()) as host, ThreadedRawEthernetClient(config(True)) as peer:
    peer.send(b"explicit-reopen")
    assert host.receive(2).payload == b"explicit-reopen"
print("installed wheel: exchange, checksum/VLAN/truncation rejection, typed link failure, explicit reopen passed")
