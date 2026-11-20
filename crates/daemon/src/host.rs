//! Facts about the machine the daemon runs on.

use std::net::{IpAddr, UdpSocket};

/// Best-effort public address: `<PREFIX>_PUBLIC_IP`, else the IPv4 address of the interface
/// that routes to the internet. No packet is sent: connecting a UDP socket only picks a route.
pub fn public_ip() -> Option<String> {
    if let Ok(ip) = std::env::var(common::brand::env_var("PUBLIC_IP"))
        && !ip.trim().is_empty()
    {
        return Some(ip.trim().to_string());
    }
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("1.1.1.1:80").ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() && !ip.is_link_local() => {
            Some(ip.to_string())
        }
        _ => None,
    }
}

pub fn hostname() -> String {
    sysinfo::System::host_name().unwrap_or_else(|| "localhost".into())
}
