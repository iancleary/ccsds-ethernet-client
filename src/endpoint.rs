use std::error::Error;
use std::fmt;
use std::net::Ipv4Addr;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Endpoint {
    ipv4: Ipv4Addr,
    udp_port: u16,
}

impl Endpoint {
    pub fn new(ipv4: Ipv4Addr, udp_port: u16) -> Result<Self, EndpointError> {
        if ipv4.is_unspecified() || ipv4.is_multicast() || ipv4.is_broadcast() || udp_port == 0 {
            return Err(EndpointError { ipv4, udp_port });
        }
        Ok(Self { ipv4, udp_port })
    }

    pub const fn ipv4(self) -> Ipv4Addr {
        self.ipv4
    }

    pub const fn udp_port(self) -> u16 {
        self.udp_port
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.ipv4, self.udp_port)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EndpointError {
    ipv4: Ipv4Addr,
    udp_port: u16,
}

impl fmt::Display for EndpointError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "endpoint must use a concrete unicast IPv4 address and nonzero UDP port, got {}:{}",
            self.ipv4, self.udp_port
        )
    }
}

impl Error for EndpointError {}
