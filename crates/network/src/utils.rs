use std::net::SocketAddr;

use libp2p::multiaddr;

pub fn multiaddr_from_socketaddr(
    addr: SocketAddr,
) -> Result<multiaddr::Multiaddr, multiaddr::Error> {
    if addr.is_ipv6() {
        format!("/ip6/{}/tcp/{}", addr.ip(), addr.port()).parse()
    } else {
        format!("/ip4/{}/tcp/{}", addr.ip(), addr.port()).parse()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multiaddr_from_socketaddr_ipv4() {
        let addr = "127.0.0.1:8080".parse().unwrap();
        let multiaddr = multiaddr_from_socketaddr(addr).unwrap();
        assert_eq!(multiaddr.to_string(), "/ip4/127.0.0.1/tcp/8080");
    }

    #[test]
    fn test_multiaddr_from_socketaddr_ipv6() {
        let addr = "[::1]:8080".parse().unwrap();
        let multiaddr = multiaddr_from_socketaddr(addr).unwrap();
        assert_eq!(multiaddr.to_string(), "/ip6/::1/tcp/8080");
    }
}
