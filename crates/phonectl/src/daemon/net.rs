//! Tailscale transport: the listener, pokes, and the network-change watch.
//!
//! The listener binds to this machine's Tailscale IPv4 address only, so it is
//! never reachable from a café or school LAN, whatever the firewall says. It
//! also refuses peers outside 100.64.0.0/10. Authentication on top of that is
//! the HMAC handshake in `session`.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use phone::link;
use tokio::io::unix::AsyncFd;
use tokio::net::{TcpListener, UdpSocket};

use super::{Shared, Transport, now_millis};

/// Tailscale's CGNAT range.
pub fn is_tailscale(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            o[0] == 100 && (64..128).contains(&o[1])
        }
        IpAddr::V6(v6) => v6.to_ipv4_mapped().is_some_and(|v4| is_tailscale(IpAddr::V4(v4))),
    }
}

/// The IPv4 address on `tailscale0`, if Tailscale is up.
pub fn tailscale_address() -> Option<Ipv4Addr> {
    let mut found = None;
    // SAFETY: getifaddrs/freeifaddrs pair; we only read the list in between.
    unsafe {
        let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut list) != 0 {
            return None;
        }
        let mut cursor = list;
        while !cursor.is_null() {
            let entry = &*cursor;
            if !entry.ifa_addr.is_null() && (*entry.ifa_addr).sa_family as i32 == libc::AF_INET {
                let sin = &*(entry.ifa_addr as *const libc::sockaddr_in);
                let ip = Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
                if is_tailscale(IpAddr::V4(ip)) {
                    found = Some(ip);
                    break;
                }
            }
            cursor = entry.ifa_next;
        }
        libc::freeifaddrs(list);
    }
    found
}

/// Binds once Tailscale has an address (retrying on network changes), then
/// accepts connections forever.
pub async fn listen(daemon: Shared) {
    let port = daemon.config.port;
    let listener = loop {
        if let Some(ip) = tailscale_address() {
            match TcpListener::bind(SocketAddr::new(IpAddr::V4(ip), port)).await {
                Ok(listener) => {
                    tracing::info!("listening on {ip}:{port}");
                    break listener;
                }
                Err(e) => tracing::warn!("cannot listen on {ip}:{port}: {e}"),
            }
        } else {
            tracing::info!("waiting for Tailscale to come up");
        }
        daemon.state.lock().unwrap().listening = false;
        daemon.publish();
        // Woken by the netlink watch, with a slow fallback.
        let mut changes = NETWORK_CHANGES.subscribe();
        let _ = tokio::time::timeout(Duration::from_secs(300), changes.recv()).await;
    };
    daemon.state.lock().unwrap().listening = true;
    daemon.publish();

    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                if !is_tailscale(peer.ip()) {
                    tracing::warn!("refusing non-Tailscale peer {peer}");
                    continue;
                }
                let _ = stream.set_nodelay(true);
                set_keepalive(&stream);
                tokio::spawn(super::session::run(daemon.clone(), stream, Transport::Tailscale, peer.to_string()));
            }
            Err(e) => {
                tracing::warn!("accept: {e}");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

/// Kernel keepalive as a backstop under the app-level ping: catches a phone
/// that vanished while we had nothing to send.
fn set_keepalive(stream: &tokio::net::TcpStream) {
    let fd = stream.as_raw_fd();
    let set = |level, name, value: libc::c_int| unsafe {
        libc::setsockopt(fd, level, name, &value as *const _ as *const libc::c_void, size_of::<libc::c_int>() as u32);
    };
    set(libc::SOL_SOCKET, libc::SO_KEEPALIVE, 1);
    set(libc::IPPROTO_TCP, libc::TCP_KEEPIDLE, 600);
    set(libc::IPPROTO_TCP, libc::TCP_KEEPINTVL, 60);
    set(libc::IPPROTO_TCP, libc::TCP_KEEPCNT, 3);
    // Give up on unacknowledged data after 2 minutes instead of ~15.
    set(libc::IPPROTO_TCP, libc::TCP_USER_TIMEOUT, 120_000);
}

static NETWORK_CHANGES: std::sync::LazyLock<tokio::sync::broadcast::Sender<()>> =
    std::sync::LazyLock::new(|| tokio::sync::broadcast::channel(4).0);

static LAST_POKE: Mutex<Option<Instant>> = Mutex::new(None);

/// Sends a few pokes spread over ~20 s (Tailscale may still be re-establishing
/// its path after a resume or network change). Skipped when connected or
/// when the phone's address is unknown. Rate limited to one burst a minute.
pub async fn poke_burst(daemon: Shared, why: &str) {
    if daemon.sessions.current().is_some() {
        return;
    }
    {
        let mut last = LAST_POKE.lock().unwrap();
        if last.is_some_and(|t| t.elapsed() < Duration::from_secs(60)) && why == "network change" {
            return;
        }
        *last = Some(Instant::now());
    }
    let Some(ip) = daemon.state.lock().unwrap().phone.tailscale_ip.clone() else {
        tracing::debug!("no phone address yet, not poking");
        return;
    };
    let Ok(ip) = ip.parse::<IpAddr>() else { return };
    tracing::info!("poking phone ({why})");
    daemon.expect_connection(Duration::from_secs(30));
    let socket = match UdpSocket::bind("0.0.0.0:0").await {
        Ok(socket) => socket,
        Err(e) => {
            tracing::warn!("poke: {e}");
            return;
        }
    };
    for delay in [0u64, 4, 15] {
        tokio::time::sleep(Duration::from_secs(delay)).await;
        if daemon.sessions.current().is_some() {
            break;
        }
        let packet = link::poke_packet(&daemon.key, now_millis() as u64);
        if let Err(e) = socket.send_to(&packet, SocketAddr::new(ip, link::POKE_PORT)).await {
            tracing::debug!("poke send: {e}");
        }
    }
}

/// Listens for link and address changes over rtnetlink. Each burst of changes
/// (debounced) pokes the phone when we are not connected, and wakes the
/// listener if it is still waiting for Tailscale.
pub async fn watch_network(daemon: Shared) {
    let fd = match open_netlink() {
        Ok(fd) => fd,
        Err(e) => {
            tracing::warn!("network change watch unavailable: {e}");
            return;
        }
    };
    let Ok(fd) = AsyncFd::new(fd) else { return };
    let mut buffer = vec![0u8; 16 * 1024];
    loop {
        let Ok(mut guard) = fd.readable().await else { return };
        let read = guard.try_io(|inner| {
            let n = unsafe { libc::recv(inner.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len(), libc::MSG_DONTWAIT) };
            if n < 0 { Err(std::io::Error::last_os_error()) } else { Ok(n as usize) }
        });
        // Err: stale readiness (cleared now); Ok(Err): e.g. ENOBUFS. Either
        // way there is no fresh change to act on.
        if !matches!(read, Ok(Ok(_))) {
            continue;
        }
        // Debounce: networks change in bursts (link up, addresses, routes).
        tokio::time::sleep(Duration::from_secs(3)).await;
        loop {
            let n = unsafe { libc::recv(fd.get_ref().as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len(), libc::MSG_DONTWAIT) };
            if n <= 0 {
                break;
            }
        }
        let _ = NETWORK_CHANGES.send(());
        tracing::debug!("network changed");
        if daemon.sessions.current().is_none() {
            tokio::spawn(poke_burst(daemon.clone(), "network change"));
        }
    }
}

fn open_netlink() -> std::io::Result<OwnedFd> {
    unsafe {
        let fd = libc::socket(libc::AF_NETLINK, libc::SOCK_RAW | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK, libc::NETLINK_ROUTE);
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let owned = OwnedFd::from_raw_fd(fd);
        let mut addr: libc::sockaddr_nl = std::mem::zeroed();
        addr.nl_family = libc::AF_NETLINK as u16;
        addr.nl_groups = (libc::RTMGRP_LINK | libc::RTMGRP_IPV4_IFADDR) as u32;
        if libc::bind(fd, &addr as *const _ as *const libc::sockaddr, size_of::<libc::sockaddr_nl>() as u32) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(owned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tailscale_range() {
        assert!(is_tailscale("100.101.102.103".parse().unwrap()));
        assert!(is_tailscale("100.64.0.1".parse().unwrap()));
        assert!(is_tailscale("100.127.255.255".parse().unwrap()));
        assert!(!is_tailscale("100.128.0.1".parse().unwrap()));
        assert!(!is_tailscale("100.63.0.1".parse().unwrap()));
        assert!(!is_tailscale("192.168.1.20".parse().unwrap()));
        assert!(is_tailscale("::ffff:100.88.77.66".parse().unwrap()));
        assert!(!is_tailscale("fd7a:115c:a1e0::1".parse().unwrap()));
    }
}
