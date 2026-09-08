use ccsds_ethernet_client::{
    Endpoint, ReceivedFrame, Transport, TransportError, TransportStatistics,
};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

pub type Link = Box<dyn Transport + Send>;
type Queue = Arc<(Mutex<QueueState>, Condvar)>;

struct QueueState {
    frames: VecDeque<ReceivedFrame>,
    capacity: usize,
    maximum: usize,
    dropped: u64,
}

struct Simulated {
    incoming: Queue,
    outgoing: Queue,
    sender: Endpoint,
    statistics: TransportStatistics,
}

impl Transport for Simulated {
    fn start_receive(&mut self) -> Result<(), TransportError> {
        self.statistics.receive_ready = true;
        Ok(())
    }
    fn send(&mut self, payload: &[u8]) -> Result<(), TransportError> {
        if !self.statistics.receive_ready {
            return Err(TransportError::NotReady);
        }
        let mut queue = self.outgoing.0.lock().unwrap();
        self.statistics.sent_frames += 1;
        if queue.frames.len() == queue.capacity {
            // Model accepted-send network loss, not local send rejection.
            queue.dropped += 1;
        } else {
            queue.frames.push_back(ReceivedFrame {
                payload: payload.to_vec(),
                sender: self.sender,
            });
            queue.maximum = queue.maximum.max(queue.frames.len());
            self.outgoing.1.notify_one();
        }
        Ok(())
    }
    fn receive(&mut self, deadline: Instant) -> Result<ReceivedFrame, TransportError> {
        if !self.statistics.receive_ready {
            return Err(TransportError::NotReady);
        }
        let mut queue = self.incoming.0.lock().unwrap();
        loop {
            if Instant::now() >= deadline {
                self.statistics.receive_timeouts += 1;
                return Err(TransportError::TimedOut);
            }
            if let Some(frame) = queue.frames.pop_front() {
                self.statistics.received_frames += 1;
                return Ok(frame);
            }
            queue = self
                .incoming
                .1
                .wait_timeout(queue, deadline.saturating_duration_since(Instant::now()))
                .unwrap()
                .0;
        }
    }
    fn statistics(&self) -> TransportStatistics {
        let queue = self.incoming.0.lock().unwrap();
        let mut stats = self.statistics;
        stats.maximum_queue_depth = queue.maximum;
        stats.queue_capacity = queue.capacity;
        stats.dropped_frames = queue.dropped;
        stats
    }
    fn close(&mut self) -> Result<(), TransportError> {
        self.statistics.receive_ready = false;
        Ok(())
    }
}

pub fn pair(backend: &str, capacity: usize) -> Result<(Link, Link), TransportError> {
    let host = Endpoint::new("192.0.2.1".parse::<std::net::Ipv4Addr>().unwrap(), 40001).unwrap();
    let peer = Endpoint::new("192.0.2.2".parse::<std::net::Ipv4Addr>().unwrap(), 40002).unwrap();
    if backend == "simulated" {
        let queue = || {
            Arc::new((
                Mutex::new(QueueState {
                    frames: VecDeque::with_capacity(capacity),
                    capacity,
                    maximum: 0,
                    dropped: 0,
                }),
                Condvar::new(),
            ))
        };
        let first = queue();
        let second = queue();
        return Ok((
            Box::new(Simulated {
                incoming: first.clone(),
                outgoing: second.clone(),
                sender: host,
                statistics: TransportStatistics::default(),
            }),
            Box::new(Simulated {
                incoming: second,
                outgoing: first,
                sender: peer,
                statistics: TransportStatistics::default(),
            }),
        ));
    }
    #[cfg(target_os = "linux")]
    if backend == "veth" && std::env::var("CCSDS_LIVE_TEST").as_deref() == Ok("1") {
        use ccsds_ethernet_client::{
            Ipv4ChecksumPolicy, LinuxRawEthernetTransport, MacAddress, RawEthernetConfig,
            RawEthernetEndpoint,
        };
        let host = RawEthernetEndpoint::new(MacAddress::new([2, 0, 0, 0, 0, 1]), host).unwrap();
        let peer = RawEthernetEndpoint::new(MacAddress::new([2, 0, 0, 0, 0, 2]), peer).unwrap();
        let config = |name, local, remote| {
            RawEthernetConfig::new(2, name, local, remote, capacity)
                .unwrap()
                .with_ipv4_checksum_policy(Ipv4ChecksumPolicy::Require)
        };
        return Ok((
            Box::new(LinuxRawEthernetTransport::open(config(
                "ccsds-host",
                host,
                peer,
            ))?),
            Box::new(LinuxRawEthernetTransport::open(config(
                "ccsds-peer",
                peer,
                host,
            ))?),
        ));
    }
    Err(TransportError::Other(
        "veth requires the isolated Linux fixture; no physical interface mode is supported".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn simulated_queue_loss_is_bounded_and_does_not_fail_accepted_send() {
        let (mut host, mut peer) = pair("simulated", 1).unwrap();
        host.start_receive().unwrap();
        peer.start_receive().unwrap();
        for id in 0..3 {
            host.send(&[id]).unwrap();
        }
        assert_eq!(host.statistics().sent_frames, 3);
        assert_eq!(peer.statistics().dropped_frames, 2);
        assert_eq!(peer.statistics().maximum_queue_depth, 1);
        assert_eq!(
            peer.receive(Instant::now() + std::time::Duration::from_secs(1))
                .unwrap()
                .payload,
            [0]
        );
        assert!(pair("physical", 1).is_err());
    }
}
