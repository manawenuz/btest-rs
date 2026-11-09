//! Resolve a client `--interface` value to a local address and, on platforms
//! that support it, pin the socket to that device.
//!
//! The value may be an interface name (`eth0`, `en0`) or a local address
//! (`10.0.0.1`). Binding the source address keeps the UDP data path on the
//! same address the server learned from the TCP control connection. Pinning
//! the device forces egress onto that NIC when the host has more than one route.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::num::NonZeroU32;

use crate::protocol::{BtestError, Result};

#[derive(Debug, Clone)]
pub struct IfaceSpec {
    pub requested: String,
    explicit_ip: Option<IpAddr>,
    addrs: Vec<IpAddr>,
    #[allow(dead_code)]
    ifindex: Option<NonZeroU32>,
    #[allow(dead_code)]
    ifname: Option<String>,
}

impl IfaceSpec {
    pub fn parse(spec: &str) -> Result<Self> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err(BtestError::Protocol(
                "interface name or local address is empty".into(),
            ));
        }
        if let Ok(ip) = spec.parse::<IpAddr>() {
            let (ifindex, ifname) = lookup_device_for_ip(ip);
            return Ok(Self {
                requested: spec.to_string(),
                explicit_ip: Some(ip),
                addrs: vec![ip],
                ifindex,
                ifname,
            });
        }
        parse_interface_name(spec)
    }

    /// Local address to bind for a destination of this family.
    pub fn local_ip_for(&self, ipv6: bool) -> Result<IpAddr> {
        if let Some(ip) = self.explicit_ip {
            if ip.is_ipv6() != ipv6 {
                return Err(BtestError::Protocol(format!(
                    "local address {} is {}, destination is {}",
                    ip,
                    if ip.is_ipv6() { "IPv6" } else { "IPv4" },
                    if ipv6 { "IPv6" } else { "IPv4" },
                )));
            }
            return Ok(ip);
        }
        select_addr(&self.addrs, ipv6).ok_or_else(|| {
            BtestError::Protocol(format!(
                "interface \"{}\" has no {} address",
                self.requested,
                if ipv6 { "IPv6" } else { "IPv4" },
            ))
        })
    }

    pub fn supports(&self, ipv6: bool) -> bool {
        self.local_ip_for(ipv6).is_ok()
    }
}

/// Prefer a routable address. Link-local is a last resort. The first address
/// of the best rank wins, so an interface with several global addresses uses
/// the first one the OS reports. Pass that address explicitly to choose another.
pub(crate) fn select_addr(addrs: &[IpAddr], ipv6: bool) -> Option<IpAddr> {
    let mut best: Option<(u8, IpAddr)> = None;
    for &ip in addrs {
        if ip.is_ipv6() != ipv6 || ip.is_unspecified() {
            continue;
        }
        let rank = addr_rank(ip);
        match best {
            Some((prev, _)) if rank >= prev => {}
            _ => best = Some((rank, ip)),
        }
    }
    best.map(|(_, ip)| ip)
}

fn addr_rank(ip: IpAddr) -> u8 {
    match ip {
        IpAddr::V4(v4) if v4.is_loopback() => 1,
        IpAddr::V4(v4) if is_v4_link_local(v4) => 2,
        IpAddr::V6(v6) if v6.is_loopback() => 1,
        IpAddr::V6(v6) if is_v6_link_local(v6) => 2,
        _ => 0,
    }
}

fn is_v4_link_local(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 169 && o[1] == 254
}

fn is_v6_link_local(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfe80
}

#[cfg(unix)]
fn parse_interface_name(name: &str) -> Result<IfaceSpec> {
    let index = if_index(name)
        .ok_or_else(|| BtestError::Protocol(format!("interface \"{name}\" not found")))?;
    let addrs = interface_addresses()?
        .into_iter()
        .filter(|entry| entry.name == name)
        .map(|entry| entry.ip)
        .collect();
    Ok(IfaceSpec {
        requested: name.to_string(),
        explicit_ip: None,
        addrs,
        ifindex: Some(index),
        ifname: Some(name.to_string()),
    })
}

#[cfg(not(unix))]
fn parse_interface_name(name: &str) -> Result<IfaceSpec> {
    Err(BtestError::Protocol(format!(
        "interface name \"{name}\" is not supported on this platform; pass a local IP address"
    )))
}

#[cfg(unix)]
fn lookup_device_for_ip(ip: IpAddr) -> (Option<NonZeroU32>, Option<String>) {
    let entries = match interface_addresses() {
        Ok(entries) => entries,
        Err(e) => {
            tracing::debug!("could not list interfaces while looking up {ip}: {e}");
            return (None, None);
        }
    };
    for entry in entries {
        if entry.ip == ip {
            let index = if_index(&entry.name).or(entry.index);
            return (index, Some(entry.name));
        }
    }
    (None, None)
}

#[cfg(not(unix))]
fn lookup_device_for_ip(_ip: IpAddr) -> (Option<NonZeroU32>, Option<String>) {
    (None, None)
}

/// Restrict `fd` to the interface. A permission failure is not fatal when the
/// socket will also bind a local address on that interface.
#[cfg(unix)]
pub fn pin_fd(fd: std::os::fd::RawFd, spec: &IfaceSpec, ipv6: bool) -> std::io::Result<()> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        if let Some(name) = &spec.ifname {
            return pin_linux(fd, name);
        }
        return Ok(());
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        if let Some(index) = spec.ifindex {
            return pin_apple(fd, index.get(), ipv6);
        }
        return Ok(());
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    )))]
    {
        let _ = (fd, spec, ipv6);
        Ok(())
    }
}

#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
fn pin_linux(fd: std::os::fd::RawFd, name: &str) -> std::io::Result<()> {
    let mut buf = name.as_bytes().to_vec();
    if buf.last() != Some(&0) {
        buf.push(0);
    }
    let rc = unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_BINDTODEVICE,
            buf.as_ptr().cast(),
            buf.len() as libc::socklen_t,
        )
    };
    if rc == 0 {
        tracing::debug!("pinned socket to device {name}");
        return Ok(());
    }
    let err = std::io::Error::last_os_error();
    if matches!(err.raw_os_error(), Some(libc::EPERM) | Some(libc::EACCES)) {
        tracing::warn!(
            "could not bind socket to device {name} ({err}); continuing with the local address only"
        );
        return Ok(());
    }
    Err(err)
}

#[cfg(all(unix, any(target_os = "macos", target_os = "ios")))]
fn pin_apple(fd: std::os::fd::RawFd, index: u32, ipv6: bool) -> std::io::Result<()> {
    let (level, optname) = if ipv6 {
        (libc::IPPROTO_IPV6, libc::IPV6_BOUND_IF)
    } else {
        (libc::IPPROTO_IP, libc::IP_BOUND_IF)
    };
    let rc = unsafe {
        libc::setsockopt(
            fd,
            level as libc::c_int,
            optname as libc::c_int,
            (&index as *const u32).cast(),
            std::mem::size_of::<u32>() as libc::socklen_t,
        )
    };
    if rc == 0 {
        tracing::debug!(
            "pinned socket to interface index {index} ({})",
            if ipv6 { "IPv6" } else { "IPv4" }
        );
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(unix)]
struct ListedAddr {
    name: String,
    ip: IpAddr,
    index: Option<NonZeroU32>,
}

#[cfg(unix)]
fn if_index(name: &str) -> Option<NonZeroU32> {
    let c_name = std::ffi::CString::new(name).ok()?;
    let index = unsafe { libc::if_nametoindex(c_name.as_ptr()) };
    NonZeroU32::new(index)
}

#[cfg(unix)]
fn interface_addresses() -> Result<Vec<ListedAddr>> {
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    let rc = unsafe { libc::getifaddrs(&mut head) };
    if rc != 0 {
        return Err(BtestError::Io(std::io::Error::last_os_error()));
    }
    let _guard = IfaddrsGuard(head);

    let mut out = Vec::new();
    let mut cursor = head;
    while !cursor.is_null() {
        let entry = unsafe { &*cursor };
        cursor = entry.ifa_next;
        if entry.ifa_name.is_null() {
            continue;
        }
        let name = unsafe { std::ffi::CStr::from_ptr(entry.ifa_name) }
            .to_string_lossy()
            .into_owned();
        let Some(ip) = sockaddr_ip(entry.ifa_addr) else {
            continue;
        };
        let index = if_index(&name);
        out.push(ListedAddr { name, ip, index });
    }
    Ok(out)
}

#[cfg(unix)]
fn sockaddr_ip(sa: *const libc::sockaddr) -> Option<IpAddr> {
    if sa.is_null() {
        return None;
    }
    unsafe {
        match (*sa).sa_family as i32 {
            libc::AF_INET => {
                let sin = &*(sa as *const libc::sockaddr_in);
                let addr = u32::from_be(sin.sin_addr.s_addr);
                Some(IpAddr::V4(Ipv4Addr::from(addr)))
            }
            libc::AF_INET6 => {
                let sin6 = &*(sa as *const libc::sockaddr_in6);
                Some(IpAddr::V6(Ipv6Addr::from(sin6.sin6_addr.s6_addr)))
            }
            _ => None,
        }
    }
}

#[cfg(unix)]
struct IfaddrsGuard(*mut libc::ifaddrs);

#[cfg(unix)]
impl Drop for IfaddrsGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { libc::freeifaddrs(self.0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_routable_address_over_link_local() {
        let addrs = [
            IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1)),
            IpAddr::V4(Ipv4Addr::new(10, 1, 1, 5)),
            IpAddr::V4(Ipv4Addr::new(192, 168, 0, 2)),
        ];
        assert_eq!(
            select_addr(&addrs, false),
            Some(IpAddr::V4(Ipv4Addr::new(10, 1, 1, 5)))
        );
    }

    #[test]
    fn falls_back_to_link_local_when_it_is_the_only_address() {
        let link_local = IpAddr::V6("fe80::1".parse().unwrap());
        assert_eq!(select_addr(&[link_local], true), Some(link_local));
        assert_eq!(select_addr(&[link_local], false), None);
    }

    #[test]
    fn explicit_address_must_match_destination_family() {
        let spec = IfaceSpec::parse("127.0.0.1").unwrap();
        assert_eq!(
            spec.local_ip_for(false).unwrap(),
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        );
        assert!(spec.local_ip_for(true).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn unknown_interface_is_rejected() {
        let err = IfaceSpec::parse("btest-no-such-iface").unwrap_err();
        assert!(
            err.to_string().contains("not found"),
            "unexpected error: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn loopback_interface_resolves_to_loopback_address() {
        let name = if cfg!(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd"
        )) {
            "lo0"
        } else {
            "lo"
        };
        let spec = IfaceSpec::parse(name).unwrap();
        let ip = spec.local_ip_for(false).unwrap();
        assert!(ip.is_loopback(), "{name} resolved to {ip}");
    }
}
