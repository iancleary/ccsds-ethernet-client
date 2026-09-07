from concurrent.futures import ThreadPoolExecutor
from queue import Empty, Queue
from threading import Event, get_ident
import unittest
from unittest.mock import patch

from ccsds_ethernet_client import ThreadedRawEthernetClient, TransportError


class FakeNative:
    def __init__(self, config):
        self.owner = get_ident()
        self.frames = Queue()
        self.sent = []
        self.closed = False

    def check_owner(self):
        assert self.owner == get_ident()

    def send(self, payload):
        self.check_owner()
        self.sent.append(payload)
        self.frames.put(payload)

    def receive(self, timeout_seconds):
        self.check_owner()
        try:
            return self.frames.get(timeout=timeout_seconds)
        except Empty:
            raise TimeoutError from None

    def statistics(self):
        self.check_owner()
        return tuple(self.sent)

    def close(self):
        self.check_owner()
        self.closed = True


class ThreadedTests(unittest.TestCase):
    def test_receive_overflow_is_counted_without_resending(self):
        processed = Event()
        class BurstNative(FakeNative):
            def statistics(self):
                result = super().statistics()
                if len(result) == 3:
                    processed.set()
                return result
        with patch("ccsds_ethernet_client.threaded.RawEthernetClient", BurstNative):
            with ThreadedRawEthernetClient(None, receive_capacity=1) as client:
                for payload in (b"first", b"second", b"third"):
                    client.send(payload)
                self.assertTrue(processed.wait(1))
                # The snapshot lock synchronizes with the enqueue/drop update.
                self.assertEqual(client.statistics(), (b"first", b"second", b"third"))
                self.assertEqual(client.dropped_datagrams, 2)
                self.assertEqual(client.receive(0), b"first")

    def client(self):
        patcher = patch("ccsds_ethernet_client.threaded.RawEthernetClient", FakeNative)
        patcher.start()
        self.addCleanup(patcher.stop)
        client = ThreadedRawEthernetClient(None)
        self.addCleanup(client.close)
        return client

    def test_send_while_receive_waits_has_one_native_owner(self):
        client = self.client()
        waiting = Event()
        original_wait = client._condition.wait
        def wait(timeout):
            waiting.set()
            return original_wait(timeout)
        with patch.object(client._condition, "wait", wait), ThreadPoolExecutor(1) as pool:
            result = pool.submit(client.receive, 1)
            self.assertTrue(waiting.wait(1))
            client.send(b"command")
            self.assertEqual(result.result(1), b"command")

    def test_cancel_wakes_waiter_and_next_receive_still_works(self):
        client = self.client()
        waiting = Event()
        original_wait = client._condition.wait
        def wait(timeout):
            waiting.set()
            return original_wait(timeout)
        with patch.object(client._condition, "wait", wait), ThreadPoolExecutor(1) as pool:
            result = pool.submit(client.receive, 60)
            self.assertTrue(waiting.wait(1))
            client.cancel_receive()
            with self.assertRaisesRegex(TransportError, "cancelled"):
                result.result(1)
        client.send(b"next")
        self.assertEqual(client.receive(1), b"next")

    def test_close_wakes_waiter_and_is_idempotent(self):
        client = self.client()
        with ThreadPoolExecutor(1) as pool:
            result = pool.submit(client.receive, 60)
            client.close()
            with self.assertRaisesRegex(TransportError, "closed"):
                result.result(1)
        client.close()
        with self.assertRaises(TransportError):
            client.send(b"late")

    def test_invalid_timeouts_and_native_start_failure(self):
        client = self.client()
        for timeout in [-1, float("nan"), float("inf")]:
            with self.assertRaises(ValueError):
                client.receive(timeout)
        with self.assertRaises(TimeoutError):
            client.receive(0)
        with patch("ccsds_ethernet_client.threaded.RawEthernetClient", side_effect=TransportError("open failed")):
            with self.assertRaisesRegex(TransportError, "open failed"):
                ThreadedRawEthernetClient(None)


if __name__ == "__main__":
    unittest.main()
