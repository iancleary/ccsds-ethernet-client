use std::error::Error;
use std::fmt;
#[rustfmt::skip]
use std::net::{
    IpAddr,
    Ipv4Addr,
    Ipv6Addr,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Endpoint {
    ip: IpAddr,
    udp_port: u16,
}

impl Endpoint {
    pub fn new(ip: impl Into<IpAddr>, udp_port: u16) -> Result<Self, EndpointError> {
        let ip = ip.into();
        let invalid_ip = match ip {
            IpAddr::V4(ipv4) => {
                ipv4.is_unspecified()
                    || ipv4.is_loopback()
                    || ipv4.is_multicast()
                    || ipv4.is_broadcast()
            }
            IpAddr::V6(ipv6) => ipv6.is_unspecified() || ipv6.is_loopback() || ipv6.is_multicast(),
        };
        if invalid_ip || udp_port == 0 {
            return Err(EndpointError { ip, udp_port });
        }
        Ok(Self { ip, udp_port })
    }

    pub const fn ip(self) -> IpAddr {
        self.ip
    }

    pub const fn as_ipv4(self) -> Option<Ipv4Addr> {
        match self.ip {
            IpAddr::V4(ipv4) => Some(ipv4),
            IpAddr::V6(_) => None,
        }
    }

    pub const fn as_ipv6(self) -> Option<Ipv6Addr> {
        match self.ip {
            IpAddr::V4(_) => None,
            IpAddr::V6(ipv6) => Some(ipv6),
        }
    }

    pub const fn udp_port(self) -> u16 {
        self.udp_port
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.ip {
            IpAddr::V4(ipv4) => write!(formatter, "{}:{}", ipv4, self.udp_port),
            IpAddr::V6(ipv6) => write!(formatter, "[{}]:{}", ipv6, self.udp_port),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EndpointError {
    ip: IpAddr,
    udp_port: u16,
}

impl fmt::Display for EndpointError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "endpoint must use a concrete unicast IP address and nonzero UDP port, got {}:{}",
            self.ip, self.udp_port
        )
    }
}

impl Error for EndpointError {}
