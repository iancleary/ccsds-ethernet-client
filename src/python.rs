use std::collections::HashMap;
use std::net::IpAddr;
use std::str::FromStr;
use std::time::{Duration, Instant};

use pyo3::create_exception;
#[cfg(target_os = "linux")]
use pyo3::exceptions::PyTimeoutError;
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyModule};

#[rustfmt::skip]
use crate::{
    Endpoint,
    FrameClassificationStatistics,
    FrameDisposition,
    IpPacketOptions,
    MacAddress,
    RAW_ETHERNET_CONFIG_SCHEMA_VERSION,
    RawEthernetConfig,
    RawEthernetEndpoint,
    TransportStatistics as RustTransportStatistics,
    build_udp_frame as rust_build_udp_frame,
    parse_udp_frame as rust_parse_udp_frame,
};
#[cfg(target_os = "linux")]
use crate::{LinuxRawEthernetTransport, Transport, TransportError as RustTransportError};

create_exception!(_native, ConfigError, PyValueError);
create_exception!(_native, FrameError, PyValueError);
create_exception!(_native, TransportError, PyRuntimeError);

fn config_error(error: impl ToString) -> PyErr {
    ConfigError::new_err(error.to_string())
}

fn frame_error(error: impl ToString) -> PyErr {
    FrameError::new_err(error.to_string())
}

#[cfg(target_os = "linux")]
fn transport_error(error: RustTransportError) -> PyErr {
    match error {
        RustTransportError::TimedOut => PyTimeoutError::new_err(error.to_string()),
        _ => TransportError::new_err(error.to_string()),
    }
}

fn endpoint(mac: &str, ip: &str, udp_port: u16) -> PyResult<RawEthernetEndpoint> {
    let mac = MacAddress::from_str(mac).map_err(config_error)?;
    let ip = IpAddr::from_str(ip).map_err(config_error)?;
    let network = Endpoint::new(ip, udp_port).map_err(config_error)?;
    RawEthernetEndpoint::new(mac, network).map_err(config_error)
}

#[pyclass(name = "RawEthernetConfig", frozen, skip_from_py_object)]
#[derive(Clone)]
struct PyRawEthernetConfig {
    inner: RawEthernetConfig,
}

#[pymethods]
impl PyRawEthernetConfig {
    #[new]
    #[pyo3(signature = (
        interface_name,
        host_mac,
        host_ip,
        host_udp_port,
        board_mac,
        board_ip,
        board_udp_port,
        ring_capacity,
        schema_version=RAW_ETHERNET_CONFIG_SCHEMA_VERSION,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        interface_name: String,
        host_mac: &str,
        host_ip: &str,
        host_udp_port: u16,
        board_mac: &str,
        board_ip: &str,
        board_udp_port: u16,
        ring_capacity: usize,
        schema_version: u16,
    ) -> PyResult<Self> {
        let host = endpoint(host_mac, host_ip, host_udp_port)?;
        let board = endpoint(board_mac, board_ip, board_udp_port)?;
        let inner =
            RawEthernetConfig::new(schema_version, interface_name, host, board, ring_capacity)
                .map_err(config_error)?;
        Ok(Self { inner })
    }

    #[getter]
    fn schema_version(&self) -> u16 {
        self.inner.schema_version()
    }

    #[getter]
    fn interface_name(&self) -> &str {
        self.inner.interface_name()
    }

    #[getter]
    fn host_mac(&self) -> String {
        self.inner.host().mac().to_string()
    }

    #[getter]
    fn host_ip(&self) -> String {
        self.inner.host().network().ip().to_string()
    }

    #[getter]
    fn host_udp_port(&self) -> u16 {
        self.inner.host().network().udp_port()
    }

    #[getter]
    fn board_mac(&self) -> String {
        self.inner.board().mac().to_string()
    }

    #[getter]
    fn board_ip(&self) -> String {
        self.inner.board().network().ip().to_string()
    }

    #[getter]
    fn board_udp_port(&self) -> u16 {
        self.inner.board().network().udp_port()
    }

    #[getter]
    fn ring_capacity(&self) -> usize {
        self.inner.ring_capacity()
    }

    #[getter]
    fn maximum_udp_payload_bytes(&self) -> usize {
        self.inner.maximum_udp_payload_bytes()
    }

    fn __repr__(&self) -> String {
        format!(
            "RawEthernetConfig(interface_name={:?}, host={:?}, board={:?}, ring_capacity={})",
            self.inner.interface_name(),
            self.inner.host().network().to_string(),
            self.inner.board().network().to_string(),
            self.inner.ring_capacity(),
        )
    }
}

#[pyclass(name = "ReceivedDatagram", frozen)]
struct PyReceivedDatagram {
    payload: Vec<u8>,
    sender: Endpoint,
}

#[pymethods]
impl PyReceivedDatagram {
    #[getter]
    fn payload<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.payload)
    }

    #[getter]
    fn sender_ip(&self) -> String {
        self.sender.ip().to_string()
    }

    #[getter]
    fn sender_udp_port(&self) -> u16 {
        self.sender.udp_port()
    }

    fn __repr__(&self) -> String {
        format!(
            "ReceivedDatagram(payload=<{} bytes>, sender={:?})",
            self.payload.len(),
            self.sender.to_string(),
        )
    }
}

#[pyclass(name = "TransportStatistics", frozen)]
struct PyTransportStatistics {
    inner: RustTransportStatistics,
}

#[pymethods]
impl PyTransportStatistics {
    #[getter]
    fn receive_ready(&self) -> bool {
        self.inner.receive_ready
    }

    #[getter]
    fn sent_frames(&self) -> u64 {
        self.inner.sent_frames
    }

    #[getter]
    fn received_frames(&self) -> u64 {
        self.inner.received_frames
    }

    #[getter]
    fn receive_timeouts(&self) -> u64 {
        self.inner.receive_timeouts
    }

    #[getter]
    fn dropped_frames(&self) -> u64 {
        self.inner.dropped_frames
    }

    #[getter]
    fn kernel_dropped_frames(&self) -> u64 {
        self.inner.kernel_dropped_frames
    }

    #[getter]
    fn ignored_outgoing_frames(&self) -> u64 {
        self.inner.ignored_outgoing_frames
    }

    #[getter]
    fn ignored_non_ipv4_frames(&self) -> u64 {
        self.inner.ignored_non_ipv4_frames
    }

    #[getter]
    fn invalid_frames(&self) -> u64 {
        self.inner.invalid_frames
    }

    #[getter]
    fn foreign_frames(&self) -> u64 {
        self.inner.foreign_frames
    }

    #[getter]
    fn maximum_queue_depth(&self) -> usize {
        self.inner.maximum_queue_depth
    }

    #[getter]
    fn queue_capacity(&self) -> usize {
        self.inner.queue_capacity
    }

    #[getter]
    fn receive_buffer_bytes(&self) -> Option<usize> {
        self.inner.receive_buffer_bytes
    }

    fn frame_classification(&self) -> HashMap<&'static str, u64> {
        let FrameClassificationStatistics {
            packet_host_frames,
            packet_broadcast_frames,
            packet_multicast_frames,
            packet_other_host_frames,
            packet_outgoing_frames,
            packet_loopback_frames,
            packet_unknown_frames,
            ethernet_header_too_short_frames,
            unsupported_ethertype_frames,
            endpoint_mismatch_frames,
            frame_too_short_failures,
            invalid_ipv4_header_failures,
            fragmented_ipv4_failures,
            invalid_ipv4_checksum_failures,
            invalid_ipv6_header_failures,
            unsupported_ipv6_extension_header_failures,
            invalid_ipv6_payload_length_failures,
            missing_ipv6_udp_checksum_failures,
            invalid_ipv6_udp_checksum_failures,
            invalid_udp_length_failures,
            invalid_udp_checksum_failures,
        } = self.inner.frame_classification;
        HashMap::from([
            ("packet_host_frames", packet_host_frames),
            ("packet_broadcast_frames", packet_broadcast_frames),
            ("packet_multicast_frames", packet_multicast_frames),
            ("packet_other_host_frames", packet_other_host_frames),
            ("packet_outgoing_frames", packet_outgoing_frames),
            ("packet_loopback_frames", packet_loopback_frames),
            ("packet_unknown_frames", packet_unknown_frames),
            (
                "ethernet_header_too_short_frames",
                ethernet_header_too_short_frames,
            ),
            ("unsupported_ethertype_frames", unsupported_ethertype_frames),
            ("endpoint_mismatch_frames", endpoint_mismatch_frames),
            ("frame_too_short_failures", frame_too_short_failures),
            ("invalid_ipv4_header_failures", invalid_ipv4_header_failures),
            ("fragmented_ipv4_failures", fragmented_ipv4_failures),
            (
                "invalid_ipv4_checksum_failures",
                invalid_ipv4_checksum_failures,
            ),
            ("invalid_ipv6_header_failures", invalid_ipv6_header_failures),
            (
                "unsupported_ipv6_extension_header_failures",
                unsupported_ipv6_extension_header_failures,
            ),
            (
                "invalid_ipv6_payload_length_failures",
                invalid_ipv6_payload_length_failures,
            ),
            (
                "missing_ipv6_udp_checksum_failures",
                missing_ipv6_udp_checksum_failures,
            ),
            (
                "invalid_ipv6_udp_checksum_failures",
                invalid_ipv6_udp_checksum_failures,
            ),
            ("invalid_udp_length_failures", invalid_udp_length_failures),
            (
                "invalid_udp_checksum_failures",
                invalid_udp_checksum_failures,
            ),
        ])
    }
}

#[pyclass(name = "RawEthernetClient")]
struct PyRawEthernetClient {
    #[cfg(target_os = "linux")]
    transport: LinuxRawEthernetTransport,
    closed: bool,
}

#[pymethods]
impl PyRawEthernetClient {
    #[new]
    fn new(config: &PyRawEthernetConfig) -> PyResult<Self> {
        #[cfg(target_os = "linux")]
        {
            let mut transport =
                LinuxRawEthernetTransport::open(config.inner.clone()).map_err(transport_error)?;
            transport.start_receive().map_err(transport_error)?;
            Ok(Self {
                transport,
                closed: false,
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = config;
            Err(pyo3::exceptions::PyNotImplementedError::new_err(
                "live raw Ethernet transport requires Linux",
            ))
        }
    }

    fn send(&mut self, py: Python<'_>, payload: &[u8]) -> PyResult<()> {
        let payload = payload.to_vec();
        #[cfg(target_os = "linux")]
        {
            py.detach(|| self.transport.send(&payload))
                .map_err(transport_error)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (py, payload);
            Err(TransportError::new_err(
                "live raw Ethernet transport requires Linux",
            ))
        }
    }

    fn receive(&mut self, py: Python<'_>, timeout_seconds: f64) -> PyResult<PyReceivedDatagram> {
        if !timeout_seconds.is_finite() || timeout_seconds < 0.0 {
            return Err(PyValueError::new_err(
                "timeout_seconds must be a finite nonnegative number",
            ));
        }
        let timeout = Duration::try_from_secs_f64(timeout_seconds)
            .map_err(|_| PyValueError::new_err("timeout_seconds is out of range"))?;
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| PyValueError::new_err("timeout_seconds is out of range"))?;
        #[cfg(target_os = "linux")]
        {
            let frame = py
                .detach(|| self.transport.receive(deadline))
                .map_err(transport_error)?;
            Ok(PyReceivedDatagram {
                payload: frame.payload,
                sender: frame.sender,
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (py, deadline);
            Err(TransportError::new_err(
                "live raw Ethernet transport requires Linux",
            ))
        }
    }

    fn statistics(&self) -> PyResult<PyTransportStatistics> {
        #[cfg(target_os = "linux")]
        {
            Ok(PyTransportStatistics {
                inner: self.transport.statistics(),
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(TransportError::new_err(
                "live raw Ethernet transport requires Linux",
            ))
        }
    }

    fn close(&mut self, py: Python<'_>) -> PyResult<()> {
        #[cfg(target_os = "linux")]
        {
            if self.closed {
                return Ok(());
            }
            let result = py.detach(|| self.transport.close());
            self.closed = true;
            result.map_err(transport_error)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = py;
            Ok(())
        }
    }

    #[getter]
    fn closed(&self) -> bool {
        self.closed
    }

    fn __enter__(self_: PyRef<'_, Self>) -> PyRef<'_, Self> {
        self_
    }

    fn __exit__(
        &mut self,
        py: Python<'_>,
        _exception_type: &Bound<'_, PyAny>,
        _exception_value: &Bound<'_, PyAny>,
        _traceback: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        self.close(py)
    }
}

#[pyfunction]
#[pyo3(signature = (config, payload, ipv4_identification=0))]
fn build_udp_frame<'py>(
    py: Python<'py>,
    config: &PyRawEthernetConfig,
    payload: &[u8],
    ipv4_identification: u16,
) -> PyResult<Bound<'py, PyBytes>> {
    let options = if config.inner.host().network().ip().is_ipv4() {
        IpPacketOptions::Ipv4 {
            identification: ipv4_identification,
        }
    } else {
        IpPacketOptions::Ipv6
    };
    let frame = rust_build_udp_frame(&config.inner, payload, options).map_err(frame_error)?;
    Ok(PyBytes::new(py, &frame))
}

#[pyfunction]
fn parse_udp_frame(
    config: &PyRawEthernetConfig,
    frame: &[u8],
) -> PyResult<Option<PyReceivedDatagram>> {
    match rust_parse_udp_frame(frame, &config.inner) {
        FrameDisposition::Matched { payload, sender } => {
            Ok(Some(PyReceivedDatagram { payload, sender }))
        }
        FrameDisposition::Foreign => Ok(None),
        FrameDisposition::Invalid(error) => Err(frame_error(error)),
    }
}

#[pymodule]
fn _native(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add("ConfigError", module.py().get_type::<ConfigError>())?;
    module.add("FrameError", module.py().get_type::<FrameError>())?;
    module.add("TransportError", module.py().get_type::<TransportError>())?;
    module.add_class::<PyRawEthernetConfig>()?;
    module.add_class::<PyReceivedDatagram>()?;
    module.add_class::<PyTransportStatistics>()?;
    module.add_class::<PyRawEthernetClient>()?;
    module.add_function(wrap_pyfunction!(build_udp_frame, module)?)?;
    module.add_function(wrap_pyfunction!(parse_udp_frame, module)?)?;
    Ok(())
}
