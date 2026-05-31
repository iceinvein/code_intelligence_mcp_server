//! Shared networking helpers for the daemon's public HTTP listeners.
//!
//! The public listeners (MCP proxy, discovery, JSON API) are recreated on every
//! daemon restart. launchd / Homebrew `KeepAlive` can relaunch the daemon while
//! the previous process's port is still lingering: long-lived MCP SSE streams
//! leave connections in `TIME_WAIT` after the old process exits. A plain
//! `TcpListener::bind` does not set `SO_REUSEADDR` on Unix, so the fresh bind
//! fails with `EADDRINUSE (os error 48)` and the listener silently runs
//! degraded (no `?repo=` binding, no `/api/*`). Binding through a `TcpSocket`
//! with `SO_REUSEADDR` enabled lets a restart reclaim the port immediately.

use std::io;
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpSocket};

/// Listen backlog for the public HTTP listeners. Most platforms clamp this to
/// `SOMAXCONN`; 1024 is comfortably above the burst of MCP client reconnects a
/// daemon restart triggers.
const LISTEN_BACKLOG: u32 = 1024;

/// Build a `TcpSocket` for `addr` with `SO_REUSEADDR` enabled.
fn reusable_socket(addr: SocketAddr) -> io::Result<TcpSocket> {
    let socket = if addr.is_ipv4() {
        TcpSocket::new_v4()?
    } else {
        TcpSocket::new_v6()?
    };
    socket.set_reuseaddr(true)?;
    Ok(socket)
}

/// Bind a TCP listener with `SO_REUSEADDR` so a daemon restart can reclaim the
/// port even while the previous process's connections linger in `TIME_WAIT`.
///
/// Must be called from within a Tokio runtime (the returned [`TcpListener`] is
/// registered with the current reactor).
pub fn bind_reusable_listener(addr: SocketAddr) -> io::Result<TcpListener> {
    let socket = reusable_socket(addr)?;
    socket.bind(addr)?;
    socket.listen(LISTEN_BACKLOG)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    #[tokio::test]
    async fn reusable_socket_enables_reuseaddr() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let socket = reusable_socket(addr).expect("socket creation should succeed");
        assert!(
            socket.reuseaddr().expect("reuseaddr is readable"),
            "listener socket must enable SO_REUSEADDR so a restart can reclaim a port \
             whose previous connections are still in TIME_WAIT"
        );
    }

    #[tokio::test]
    async fn bind_reusable_listener_returns_a_listening_socket() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = bind_reusable_listener(addr).expect("bind should succeed");
        let local = listener.local_addr().expect("local_addr is readable");
        assert_eq!(local.ip().to_string(), "127.0.0.1");
        assert_ne!(local.port(), 0, "an ephemeral port must be assigned");
    }
}
