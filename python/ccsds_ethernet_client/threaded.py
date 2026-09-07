"""Explicit single-owner transport for concurrent orchestrator threads."""

from collections import deque
from concurrent.futures import Future
import math
from queue import Empty, Full, Queue
from threading import Condition, Event, Thread
from time import monotonic

from ._native import RawEthernetClient, RawEthernetConfig, ReceivedDatagram, TransportError, TransportStatistics


class ThreadedRawEthernetClient:
    """Own the native client on one thread; callers may send and receive concurrently.

    Reception is continuous. On overflow, the oldest queued packets are retained
    and new packets are counted as dropped. This class performs no correlation,
    retries, or interpretation. Use close() or a context manager to join the owner.
    """

    def __init__(self, config: RawEthernetConfig, *, receive_capacity: int = 64, send_capacity: int = 64):
        for capacity in (receive_capacity, send_capacity):
            if not isinstance(capacity, int) or isinstance(capacity, bool) or capacity < 1:
                raise ValueError("queue capacities must be positive integers")
        self._condition = Condition()
        self._packets = deque()
        self._receive_capacity = receive_capacity
        self._commands = Queue(maxsize=send_capacity)
        self._stop = Event()
        self._ready = Event()
        self._error = None
        self._statistics = None
        self._dropped = 0
        self._cancel_generation = 0
        self._thread = Thread(target=self._run, args=(config,), name="ccsds-transport", daemon=True)
        self._thread.start()
        self._ready.wait()
        if self._error is not None:
            self._thread.join()
            raise self._error

    def _run(self, config):
        client = None
        try:
            client = RawEthernetClient(config)
            with self._condition:
                self._statistics = client.statistics()
            self._ready.set()
            while not self._stop.is_set():
                try:
                    payload, future = self._commands.get_nowait()
                except Empty:
                    pass
                else:
                    try:
                        client.send(payload)
                    except Exception as error:
                        future.set_exception(error)
                    else:
                        future.set_result(None)
                packet = None
                try:
                    packet = client.receive(timeout_seconds=0.01)
                except TimeoutError:
                    pass
                with self._condition:
                    self._statistics = client.statistics()
                    if packet is not None:
                        if len(self._packets) < self._receive_capacity:
                            self._packets.append(packet)
                            self._condition.notify_all()
                        else:
                            self._dropped += 1
        except Exception as error:
            with self._condition:
                self._error = error
        finally:
            self._stop.set()
            if client is not None:
                try:
                    client.close()
                except Exception as error:
                    with self._condition:
                        if self._error is None:
                            self._error = error
            with self._condition:
                while True:
                    try:
                        _, future = self._commands.get_nowait()
                    except Empty:
                        break
                    future.set_exception(TransportError("client stopped before send"))
                self._condition.notify_all()
            self._ready.set()

    def send(self, payload: bytes) -> None:
        if not isinstance(payload, bytes):
            raise TypeError("payload must be bytes")
        future = Future()
        with self._condition:
            self._check_open()
            try:
                self._commands.put_nowait((payload, future))
            except Full:
                raise TransportError("send queue is full; payload was not queued") from None
        future.result()

    def receive(self, timeout_seconds: float) -> ReceivedDatagram:
        if not math.isfinite(timeout_seconds) or timeout_seconds < 0:
            raise ValueError("timeout_seconds must be finite and nonnegative")
        deadline = monotonic() + timeout_seconds
        with self._condition:
            generation = self._cancel_generation
            while True:
                self._check_open()
                if generation != self._cancel_generation:
                    raise TransportError("receive cancelled")
                if self._packets:
                    return self._packets.popleft()
                remaining = deadline - monotonic()
                if remaining <= 0:
                    raise TimeoutError("receive deadline expired")
                # Keep very large caller timeouts within threading's platform limit.
                self._condition.wait(min(remaining, 60.0))

    def cancel_receive(self) -> None:
        """Wake current receive waiters; future receive calls remain usable."""
        with self._condition:
            self._cancel_generation += 1
            self._condition.notify_all()

    def statistics(self) -> TransportStatistics:
        """Return the last owner snapshot; native timeouts include idle polling."""
        with self._condition:
            return self._statistics

    @property
    def dropped_datagrams(self) -> int:
        with self._condition:
            return self._dropped

    @property
    def closed(self) -> bool:
        return self._stop.is_set()

    def _check_open(self):
        if self._stop.is_set():
            if self._error is not None:
                raise self._error
            raise TransportError("client is closed")

    def close(self) -> None:
        self._stop.set()
        with self._condition:
            self._condition.notify_all()
        self._thread.join()
        if self._error is not None:
            raise self._error

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_value, traceback):
        self.close()
