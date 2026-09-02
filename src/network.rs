use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use anyhow::Result;

use crate::config::{Config, ListenMode};

pub fn is_tailscale_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(value) => {
            let octets = value.octets();
            (octets[0] == 100) && (64..=127).contains(&octets[1])
        }
        IpAddr::V6(value) => {
            let segments = value.segments();
            segments[0] == 0xfd7a && segments[1] == 0x115c && segments[2] == 0xa1e0
        }
    }
}

pub fn listener_addresses(config: &Config) -> Vec<SocketAddr> {
    let mut addresses = vec![SocketAddr::new(
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        config.port,
    )];

    if config.listen == ListenMode::Tailnet {
        if let Ok(interfaces) = if_addrs::get_if_addrs() {
            for interface in interfaces {
                let ip = interface.ip();
                if is_tailscale_ip(ip) {
                    let address = SocketAddr::new(ip, config.port);
                    if !addresses.contains(&address) {
                        addresses.push(address);
                    }
                }
            }
        }
    }

    addresses
}

pub fn tailnet_addresses() -> Result<Vec<IpAddr>> {
    let mut addresses = Vec::new();
    for interface in if_addrs::get_if_addrs()? {
        let ip = interface.ip();
        if is_tailscale_ip(ip) && !addresses.contains(&ip) {
            addresses.push(ip);
        }
    }
    Ok(addresses)
}
