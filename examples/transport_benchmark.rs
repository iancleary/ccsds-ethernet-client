//! Workload generator and synthetic peer, not an application command protocol.
#[path = "benchmark_support/link.rs"]
mod link;
#[path = "benchmark_support/meter.rs"]
mod meter;

use ccsds_ethernet_client::{TransportError, TransportStatistics};
use std::sync::{
    Arc, Barrier,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[global_allocator]
static ALLOCATOR: meter::Meter = meter::Meter;
const HEADER: usize = 29;
const HELP: &str = "transport_benchmark [--backend simulated|veth] [--rates 100,1000,10000] [--samples 200] [--payload-bytes 64] [--burst 1] [--capacity 64] [--telemetry-per-command 1] [--telemetry-rate 0] [--peer-delay-us 0] [--drop-every 0] [--drain-ms 500]\nOutputs schema-versioned JSON Lines. veth requires the disposable Linux fixture. No physical device is selected. Use a release build; results include this generator and synthetic peer. Bounds and field definitions: docs/benchmark.md.";

#[derive(Clone, Debug)]
struct Config {
    backend: String,
    rates: Vec<u64>,
    samples: usize,
    payload: usize,
    burst: usize,
    capacity: usize,
    telemetry: usize,
    telemetry_rate: u64,
    delay_us: u64,
    drop_every: usize,
    drain_ms: u64,
}

impl Config {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut result = Self {
            backend: "simulated".into(),
            rates: vec![100, 1000, 10000],
            samples: 200,
            payload: 64,
            burst: 1,
            capacity: 64,
            telemetry: 1,
            telemetry_rate: 0,
            delay_us: 0,
            drop_every: 0,
            drain_ms: 500,
        };
        if args.len() % 2 != 0 {
            return Err("every option requires one value".into());
        }
        let number = |value: &str| {
            value
                .parse::<u64>()
                .map_err(|_| format!("invalid unsigned integer: {value}"))
        };
        for pair in args.chunks_exact(2) {
            let value = &pair[1];
            match pair[0].as_str() {
                "--backend" => result.backend = value.clone(),
                "--rates" => {
                    result.rates = value.split(',').map(number).collect::<Result<_, _>>()?
                }
                "--samples" => {
                    result.samples =
                        usize::try_from(number(value)?).map_err(|_| "samples overflow")?
                }
                "--payload-bytes" => {
                    result.payload =
                        usize::try_from(number(value)?).map_err(|_| "payload overflow")?
                }
                "--burst" => {
                    result.burst = usize::try_from(number(value)?).map_err(|_| "burst overflow")?
                }
                "--capacity" => {
                    result.capacity =
                        usize::try_from(number(value)?).map_err(|_| "capacity overflow")?
                }
                "--telemetry-per-command" => {
                    result.telemetry =
                        usize::try_from(number(value)?).map_err(|_| "telemetry overflow")?
                }
                "--peer-delay-us" => result.delay_us = number(value)?,
                "--telemetry-rate" => result.telemetry_rate = number(value)?,
                "--drop-every" => {
                    result.drop_every =
                        usize::try_from(number(value)?).map_err(|_| "drop interval overflow")?
                }
                "--drain-ms" => result.drain_ms = number(value)?,
                key => return Err(format!("unknown option: {key}")),
            }
        }
        if !matches!(result.backend.as_str(), "simulated" | "veth")
            || !(1..=100_000).contains(&result.samples)
            || !(HEADER..=1472).contains(&result.payload)
            || !(1..=result.samples).contains(&result.burst)
            || !(1..=65536).contains(&result.capacity)
            || result.telemetry > 8
            || result.telemetry_rate > 1_000_000
            || result.delay_us > 100_000
            || !(1..=10_000).contains(&result.drain_ms)
            || result.rates.is_empty()
            || result.rates.len() > 32
            || result
                .rates
                .iter()
                .any(|rate| !(1..=1_000_000).contains(rate))
            || result.rates.windows(2).any(|rates| rates[0] >= rates[1])
            || result
                .rates
                .iter()
                .any(|rate| result.samples as u64 > rate * 300)
        {
            return Err("workload outside documented bounds; use --help".into());
        }
        if result.rates.iter().any(|rate| {
            (result.samples as u64 * result.telemetry_rate).div_ceil(*rate)
                + (result.samples * result.telemetry) as u64
                > 1_000_000
        }) {
            return Err("workload exceeds one million telemetry observations per trial".into());
        }
        Ok(result)
    }

    fn background_count(&self, rate: u64) -> usize {
        (self.samples as u64 * self.telemetry_rate).div_ceil(rate) as usize
    }
}

fn packet(kind: u8, epoch: u64, sequence: u64, timestamp: u64, size: usize) -> Vec<u8> {
    let mut bytes = vec![0; size];
    bytes[..4].copy_from_slice(b"CEB1");
    bytes[4] = kind;
    bytes[5..13].copy_from_slice(&epoch.to_be_bytes());
    bytes[13..21].copy_from_slice(&sequence.to_be_bytes());
    bytes[21..29].copy_from_slice(&timestamp.to_be_bytes());
    bytes
}

fn decode(bytes: &[u8], epoch: u64) -> Option<(u8, u64, u64)> {
    if bytes.len() < HEADER || &bytes[..4] != b"CEB1" {
        return None;
    }
    let integer = |offset| u64::from_be_bytes(bytes[offset..offset + 8].try_into().unwrap());
    (integer(5) == epoch).then(|| (bytes[4], integer(13), integer(21)))
}

#[derive(Default)]
struct PeerResult {
    commands: usize,
    intentional_drops: usize,
    send_errors: usize,
    receive_errors: usize,
    close_errors: usize,
    telemetry_attempted: usize,
    background_lateness_ns: u64,
}

fn spawn_peer(
    mut peer: link::Link,
    config: Config,
    epoch: u64,
    origin: Instant,
    stop: Arc<AtomicBool>,
    rate: u64,
    gate: Arc<Barrier>,
) -> thread::JoinHandle<(PeerResult, TransportStatistics)> {
    thread::spawn(move || {
        let mut result = PeerResult::default();
        let mut background = 0;
        let background_count = config.background_count(rate);
        gate.wait();
        while !stop.load(Ordering::Relaxed) {
            let mut deadline = Instant::now() + Duration::from_millis(10);
            if background < background_count {
                let target = origin
                    + Duration::from_nanos(
                        background as u64 * 1_000_000_000 / config.telemetry_rate,
                    );
                if Instant::now() >= target {
                    result.background_lateness_ns = result
                        .background_lateness_ns
                        .max(target.elapsed().as_nanos() as u64);
                    let telemetry = packet(
                        3,
                        epoch,
                        (config.samples * config.telemetry + background) as u64,
                        origin.elapsed().as_nanos() as u64,
                        config.payload,
                    );
                    result.telemetry_attempted += 1;
                    if peer.send(&telemetry).is_err() {
                        result.send_errors += 1;
                    }
                    background += 1;
                }
                if background < background_count {
                    deadline = deadline.min(
                        origin
                            + Duration::from_nanos(
                                background as u64 * 1_000_000_000 / config.telemetry_rate,
                            ),
                    );
                }
            }
            let frame = match peer.receive(deadline) {
                Ok(frame) => frame,
                Err(TransportError::TimedOut) => continue,
                Err(_) => {
                    result.receive_errors += 1;
                    break;
                }
            };
            let Some((1, sequence, timestamp)) = decode(&frame.payload, epoch) else {
                continue;
            };
            result.commands += 1;
            if config.delay_us != 0 {
                thread::sleep(Duration::from_micros(config.delay_us));
            }
            for index in 0..config.telemetry {
                let telemetry = packet(
                    3,
                    epoch,
                    sequence * config.telemetry as u64 + index as u64,
                    origin.elapsed().as_nanos() as u64,
                    config.payload,
                );
                result.telemetry_attempted += 1;
                if peer.send(&telemetry).is_err() {
                    result.send_errors += 1;
                }
            }
            if config.drop_every != 0 && (sequence + 1) % config.drop_every as u64 == 0 {
                result.intentional_drops += 1;
            } else if peer
                .send(&packet(2, epoch, sequence, timestamp, config.payload))
                .is_err()
            {
                result.send_errors += 1;
            }
        }
        let statistics = peer.statistics();
        if peer.close().is_err() {
            result.close_errors += 1;
        }
        (result, statistics)
    })
}

#[derive(Default)]
struct Observations {
    latency: Vec<u64>,
    age: Vec<u64>,
    seen: Vec<bool>,
    issued: Vec<Option<u64>>,
    telemetry_seen: Vec<bool>,
    duplicates: usize,
    reordered: usize,
    invalid: usize,
    highest: Option<usize>,
}

impl Observations {
    fn new(config: &Config, background_count: usize) -> Self {
        Self {
            latency: Vec::with_capacity(config.samples),
            age: Vec::with_capacity(config.samples * config.telemetry + background_count),
            seen: vec![false; config.samples],
            issued: vec![None; config.samples],
            telemetry_seen: vec![false; config.samples * config.telemetry + background_count],
            ..Self::default()
        }
    }
    fn observe(&mut self, bytes: &[u8], epoch: u64, now: u64) {
        let Some((kind, sequence, timestamp)) = decode(bytes, epoch) else {
            self.invalid += 1;
            return;
        };
        if timestamp > now {
            self.invalid += 1;
            return;
        }
        let Ok(index) = usize::try_from(sequence) else {
            self.invalid += 1;
            return;
        };
        if kind == 2 && self.issued.get(index).copied().flatten() != Some(timestamp) {
            self.invalid += 1;
            return;
        }
        let seen = match kind {
            2 => self.seen.get_mut(index),
            3 => self.telemetry_seen.get_mut(index),
            _ => None,
        };
        let Some(seen) = seen else {
            self.invalid += 1;
            return;
        };
        if *seen {
            self.duplicates += 1;
            return;
        }
        *seen = true;
        if kind == 2 {
            if self.highest.is_some_and(|highest| index < highest) {
                self.reordered += 1;
            }
            self.highest = Some(self.highest.map_or(index, |highest| highest.max(index)));
            self.latency.push(now - timestamp);
        } else {
            self.age.push(now - timestamp);
        }
    }
}

fn distribution(values: &mut [u64]) -> String {
    if values.is_empty() {
        return "null".into();
    }
    values.sort_unstable();
    let percentile = |percent: usize| values[(values.len() * percent).div_ceil(100) - 1];
    format!(
        "{{\"samples\":{},\"p50_ns\":{},\"p95_ns\":{},\"p99_ns\":{},\"max_ns\":{}}}",
        values.len(),
        percentile(50),
        percentile(95),
        percentile(99),
        values[values.len() - 1]
    )
}

fn queue_json(stats: TransportStatistics) -> String {
    format!(
        "{{\"capacity\":{},\"high_water\":{},\"user_drops\":{},\"kernel_drops\":{},\"statistics_failures\":{}}}",
        stats.queue_capacity,
        stats.maximum_queue_depth,
        stats.dropped_frames,
        stats.kernel_dropped_frames,
        stats.statistics_failures
    )
}

fn reopen_probe(config: &Config) -> Result<u64, String> {
    let start = Instant::now();
    let (mut host, mut peer) =
        link::pair(&config.backend, config.capacity).map_err(|e| e.to_string())?;
    host.start_receive().map_err(|e| e.to_string())?;
    peer.start_receive().map_err(|e| e.to_string())?;
    let request = packet(1, 0, 0, 0, config.payload);
    let deadline = Instant::now() + Duration::from_secs(1);
    host.send(&request).map_err(|e| e.to_string())?;
    let received = peer.receive(deadline).map_err(|e| e.to_string())?;
    peer.send(&received.payload).map_err(|e| e.to_string())?;
    if host.receive(deadline).map_err(|e| e.to_string())?.payload != request {
        return Err("reopen probe mismatch".into());
    }
    let elapsed = start.elapsed().as_nanos() as u64;
    host.close().map_err(|e| e.to_string())?;
    peer.close().map_err(|e| e.to_string())?;
    Ok(elapsed)
}

fn trial(config: &Config, rate: u64) -> Result<(String, bool), String> {
    let (mut host, mut peer) =
        link::pair(&config.backend, config.capacity).map_err(|e| e.to_string())?;
    host.start_receive().map_err(|e| e.to_string())?;
    peer.start_receive().map_err(|e| e.to_string())?;
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos() as u64;
    let background_count = config.background_count(rate);
    let mut observations = Observations::new(config, background_count);
    let stop = Arc::new(AtomicBool::new(false));
    let origin = Instant::now();
    let gate = Arc::new(Barrier::new(2));
    let worker = spawn_peer(
        peer,
        config.clone(),
        epoch,
        origin,
        stop.clone(),
        rate,
        gate.clone(),
    );
    let accounting_start = Instant::now();
    let cpu_start = meter::cpu_ns();
    let allocation_start = meter::allocations();
    gate.wait();
    let mut offered = 0;
    let mut sent = 0;
    let mut send_errors = 0;
    let mut receive_errors = 0;
    let mut lateness_ns = 0;
    let mut last_send_ns = 0;
    let mut first_batch_ns = 0;
    let mut drain_until = None;
    loop {
        let now = Instant::now();
        let target = origin + Duration::from_nanos(offered as u64 * 1_000_000_000 / rate);
        if offered < config.samples && now >= target {
            lateness_ns = lateness_ns.max(now.duration_since(target).as_nanos() as u64);
            for _ in 0..config.burst.min(config.samples - offered) {
                let timestamp = origin.elapsed().as_nanos() as u64;
                if host
                    .send(&packet(1, epoch, offered as u64, timestamp, config.payload))
                    .is_ok()
                {
                    observations.issued[offered] = Some(timestamp);
                    sent += 1;
                } else {
                    send_errors += 1;
                }
                offered += 1;
            }
            last_send_ns = origin.elapsed().as_nanos() as u64;
            if offered == config.burst {
                first_batch_ns = last_send_ns;
            }
            if offered == config.samples {
                let background_end = if background_count == 0 {
                    origin
                } else {
                    origin
                        + Duration::from_nanos(
                            (background_count - 1) as u64 * 1_000_000_000 / config.telemetry_rate,
                        )
                };
                drain_until = Some(
                    Instant::now().max(background_end) + Duration::from_millis(config.drain_ms),
                );
            }
        }
        if drain_until.is_some_and(|deadline| Instant::now() >= deadline) {
            break;
        }
        let deadline = drain_until
            .unwrap_or(origin + Duration::from_nanos(offered as u64 * 1_000_000_000 / rate))
            .min(Instant::now() + Duration::from_millis(10));
        match host.receive(deadline) {
            Ok(frame) => {
                observations.observe(&frame.payload, epoch, origin.elapsed().as_nanos() as u64)
            }
            Err(TransportError::TimedOut) => {}
            Err(_) => {
                receive_errors += 1;
                break;
            }
        }
        if offered == config.samples
            && observations.latency.len() == sent
            && observations.age.len() == sent * config.telemetry + background_count
        {
            break;
        }
    }
    let measurement_ns = origin.elapsed().as_nanos() as u64;
    stop.store(true, Ordering::Relaxed);
    let peer_result = worker.join().map_err(|_| "synthetic peer panicked")?;
    let allocation_end = meter::allocations();
    let cpu_end = meter::cpu_ns();
    let accounting_ns = accounting_start.elapsed().as_nanos() as u64;
    let stats = host.statistics();
    host.close().map_err(|e| e.to_string())?;
    let reopen_result = reopen_probe(config);
    let reopen_failed = reopen_result.is_err();
    if let Err(error) = &reopen_result {
        eprintln!("benchmark reopen probe: {error}");
    }
    let reopen = reopen_result.map_or("null".into(), |value| value.to_string());
    let missing = sent.saturating_sub(observations.latency.len());
    let missing_telemetry =
        (sent * config.telemetry + background_count).saturating_sub(observations.age.len());
    let achieved_rate = if offered > config.burst && last_send_ns > first_batch_ns {
        format!(
            "{:.3}",
            (offered - config.burst) as f64 * 1e9 / (last_send_ns - first_batch_ns) as f64
        )
    } else {
        "null".into()
    };
    let no_loss = missing == 0
        && !reopen_failed
        && missing_telemetry == 0
        && stats.dropped_frames == 0
        && stats.kernel_dropped_frames == 0
        && peer_result.1.dropped_frames == 0
        && peer_result.1.kernel_dropped_frames == 0
        && stats.statistics_failures == 0
        && peer_result.1.statistics_failures == 0
        && send_errors == 0
        && receive_errors == 0
        && peer_result.0.send_errors == 0
        && peer_result.0.receive_errors == 0
        && peer_result.0.close_errors == 0;
    let cpu = cpu_start
        .zip(cpu_end)
        .map(|(start, end)| end.saturating_sub(start).to_string())
        .unwrap_or("null".into());
    let latency = distribution(&mut observations.latency);
    let age = distribution(&mut observations.age);
    Ok((
        format!(
            concat!(
                "{{\"schema_version\":1,\"type\":\"trial\",\"backend\":\"{}\",\"os\":\"{}\",\"arch\":\"{}\",\"debug_build\":{},",
                "\"offered_rate_per_s\":{},\"samples\":{},\"payload_bytes\":{},\"burst\":{},\"telemetry_per_command\":{},\"peer_delay_us\":{},\"drop_every\":{},\"drain_ms\":{},",
                "\"offered\":{},\"sent\":{},\"send_errors\":{},\"receive_errors\":{},\"peer_commands\":{},\"intentional_ack_drops\":{},\"peer_send_errors\":{},\"peer_receive_errors\":{},",
                "\"missing_acks\":{},\"missing_telemetry\":{},\"duplicate_packets\":{},\"reordered_acks\":{},\"invalid_packets\":{},\"issue_span_ns\":{},\"measurement_ns\":{},\"max_schedule_lateness_ns\":{},",
                "\"achieved_offer_rate_per_s\":{},\"accounting_ns\":{},\"crate_version\":\"{}\",",
                "\"background_telemetry_rate_per_s\":{},\"background_telemetry_planned\":{},\"peer_telemetry_attempted\":{},\"max_background_lateness_ns\":{},\"peer_close_errors\":{},",
                "\"cpu_process_ns\":{},\"allocation_calls\":{},\"allocation_requested_bytes\":{},\"command_rtt\":{},\"telemetry_age\":{},\"host_queue\":{},\"peer_queue\":{},\"reopen_to_echo_ns\":{}}}"
            ),
            config.backend,
            std::env::consts::OS,
            std::env::consts::ARCH,
            cfg!(debug_assertions),
            rate,
            config.samples,
            config.payload,
            config.burst,
            config.telemetry,
            config.delay_us,
            config.drop_every,
            config.drain_ms,
            offered,
            sent,
            send_errors,
            receive_errors,
            peer_result.0.commands,
            peer_result.0.intentional_drops,
            peer_result.0.send_errors,
            peer_result.0.receive_errors,
            missing,
            missing_telemetry,
            observations.duplicates,
            observations.reordered,
            observations.invalid,
            last_send_ns,
            measurement_ns,
            lateness_ns,
            achieved_rate,
            accounting_ns,
            env!("CARGO_PKG_VERSION"),
            config.telemetry_rate,
            background_count,
            peer_result.0.telemetry_attempted,
            peer_result.0.background_lateness_ns,
            peer_result.0.close_errors,
            cpu,
            allocation_end.0.saturating_sub(allocation_start.0),
            allocation_end.1.saturating_sub(allocation_start.1),
            latency,
            age,
            queue_json(stats),
            queue_json(peer_result.1),
            reopen
        ),
        no_loss,
    ))
}

fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["--help"] {
        println!("{HELP}");
        return Ok(());
    }
    let config = Config::parse(&args)?;
    let mut highest = None;
    let mut first_loss = None;
    for &rate in &config.rates {
        let (json, no_loss) = trial(&config, rate)?;
        println!("{json}");
        if no_loss {
            highest = Some(rate);
        } else if first_loss.is_none() {
            first_loss = Some(rate);
        }
    }
    let value = |value: Option<u64>| value.map_or("null".into(), |v| v.to_string());
    println!(
        "{{\"schema_version\":1,\"type\":\"sweep_summary\",\"highest_tested_rate_without_observed_loss\":{},\"first_tested_rate_with_observed_loss_or_error\":{},\"fault_injection_enabled\":{}}}",
        value(highest),
        value(first_loss),
        config.drop_every != 0
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("benchmark: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_workloads_are_rejected() {
        for args in [
            ["--samples", "0"],
            ["--rates", "100,1"],
            ["--payload-bytes", "28"],
            ["--backend", "physical"],
            ["--capacity", "65537"],
        ] {
            assert!(Config::parse(&args.map(String::from)).is_err());
        }
        assert!(Config::parse(&["--samples".into()]).is_err());
    }
    #[test]
    fn observations_separate_duplicates_reordering_and_foreign_epochs() {
        let config = Config::parse(&[]).unwrap();
        let mut result = Observations::new(&config, 0);
        result.issued[1] = Some(10);
        result.issued[2] = Some(10);
        for seq in [2, 1, 1] {
            result.observe(&packet(2, 7, seq, 10, 32), 7, 50);
        }
        result.observe(&packet(2, 8, 0, 10, 32), 7, 50);
        result.observe(&packet(3, 7, 0, 20, 32), 7, 50);
        assert_eq!(result.latency, [40, 40]);
        assert_eq!(result.age, [30]);
        assert_eq!(
            (result.duplicates, result.reordered, result.invalid),
            (1, 1, 1)
        );
        assert_eq!(distribution(&mut []), "null");
        assert!(distribution(&mut [10, 30, 20]).contains("\"p50_ns\":20"));
    }
}
