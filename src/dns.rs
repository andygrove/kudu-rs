use std::cmp::Ordering;
use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};

use ifaces;
use kudu_pb::common::HostPortPB;

lazy_static! {
    static ref LOCAL_ADDRS: HashSet<IpAddr> = {
        let mut addrs = HashSet::new();

        match ifaces::Interface::get_all() {
            Ok(ifaces) => {
                for iface in ifaces {
                    if let Some(addr) = iface.addr {
                        addrs.insert(addr.ip());
                    }
                }
            },
            Err(error) => {
                warn!("failed to resolve local interface addresses: {}", error);
            },
        }
        addrs
    };
}

/// Resolves a sequence of hostnames into a set of socket addresses. If the hostname DNS lookup
/// fails, it is filtered from the result. The results are passed to the provided callback upon
/// completion.
pub fn resolve_hosts(hostports: &[HostPortPB]) -> HashSet<SocketAddr> {
    let mut addrs = HashSet::new();
    for hostport in hostports {
        match (hostport.get_host(), hostport.get_port() as u16).to_socket_addrs() {
            Ok(resolved_addrs) => addrs.extend(resolved_addrs),
            Err(error) => warn!("unable to resolve host '{}': {}",
                                hostport.get_host(), error),
        }
    }
    addrs
}

pub fn resolve_hostports(hostports: &[(String, u16)]) -> Vec<SocketAddr> {
    let mut addrs = Vec::new();
    for &(ref host, port) in hostports {
        match (host.as_str(), port).to_socket_addrs() {
            Ok(resolved_addrs) => addrs.extend(resolved_addrs),
            Err(error) => warn!("unable to resolve hostname '{:?}': {}", host, error),
        }
    }
    addrs.sort_by(cmp_socket_addrs);
    addrs.dedup();
    addrs
}

fn cmp_socket_addrs(a: &SocketAddr, b: &SocketAddr) -> Ordering {
    match (a, b) {
        (&SocketAddr::V4(ref a), &SocketAddr::V4(ref b)) => (a.ip(), a.port()).cmp(&(b.ip(), b.port())),
        (&SocketAddr::V6(ref a), &SocketAddr::V6(ref b)) => (a.ip(), a.port()).cmp(&(b.ip(), b.port())),
        (&SocketAddr::V4(_), &SocketAddr::V6(_)) => Ordering::Less,
        (&SocketAddr::V6(_), &SocketAddr::V4(_)) => Ordering::Greater,
    }
}

/// Returns `true` if socket addr is for a local interface.
pub fn is_local_addr(addr: &IpAddr) -> bool {
    LOCAL_ADDRS.contains(addr) || match *addr {
        // TODO: unwrapping will be unnecessary once 1.12 lands in stable.
        IpAddr::V4(ref addr) => addr.is_loopback(),
        IpAddr::V6(ref addr) => addr.is_loopback(),
    }
}

#[cfg(test)]
mod tests {

    use std::net::ToSocketAddrs;
    use super::*;

    #[test]
    fn test_is_local_addr() {
        let addr = "127.0.1.1:0".to_socket_addrs().unwrap().next().unwrap().ip();
        assert!(is_local_addr(&addr));
        let addr = "127.0.0.1:0".to_socket_addrs().unwrap().next().unwrap().ip();
        assert!(is_local_addr(&addr));
    }
}
