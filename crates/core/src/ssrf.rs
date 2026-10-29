use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Webhook targets are user-supplied URLs, which makes the server an SSRF proxy
/// unless private and link-local destinations are refused.
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_v4(v4),
        IpAddr::V6(v6) => is_blocked_v6(v6),
    }
}

fn is_blocked_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();

    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || ip.is_multicast()
        // Carrier-grade NAT.
        || (a == 100 && (64..=127).contains(&b))
        // "This network" and IETF protocol assignments.
        || a == 0
        || (a == 192 && b == 0)
        // Benchmarking range, machines on an internal fabric often live here.
        || (a == 198 && (b == 18 || b == 19))
}

fn is_blocked_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_blocked_v4(v4);
    }

    let segments = ip.segments();
    let is_unique_local = segments[0] & 0xfe00 == 0xfc00;
    let is_link_local = segments[0] & 0xffc0 == 0xfe80;
    let is_documentation = segments[0] == 0x2001 && segments[1] == 0x0db8;

    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || is_unique_local
        || is_link_local
        || is_documentation
    // IPv4-compatible (deprecated) and NAT64 can reach v4 space indirectly.
        || ip.to_ipv4().is_some_and(is_blocked_v4)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocked(ip: &str) -> bool {
        is_blocked_ip(ip.parse().unwrap())
    }

    #[test]
    fn blocks_loopback_and_private_v4() {
        for ip in [
            "127.0.0.1",
            "127.10.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "0.0.0.0",
        ] {
            assert!(blocked(ip), "expected {ip} to be blocked");
        }
    }

    #[test]
    fn blocks_metadata_and_special_ranges() {
        for ip in [
            "169.254.169.254",
            "100.64.0.1",
            "192.0.0.1",
            "198.18.0.1",
            "224.0.0.1",
            "255.255.255.255",
            "192.0.2.10",
        ] {
            assert!(blocked(ip), "expected {ip} to be blocked");
        }
    }

    #[test]
    fn blocks_loopback_and_private_v6() {
        for ip in ["::1", "::", "fc00::1", "fd12:3456::1", "fe80::1", "ff02::1"] {
            assert!(blocked(ip), "expected {ip} to be blocked");
        }
    }

    #[test]
    fn blocks_ipv4_mapped_private_addresses() {
        for ip in [
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::ffff:169.254.169.254",
        ] {
            assert!(blocked(ip), "expected {ip} to be blocked");
        }
    }

    #[test]
    fn allows_public_addresses() {
        for ip in [
            "8.8.8.8",
            "1.1.1.1",
            "93.184.216.34",
            "2606:4700:4700::1111",
        ] {
            assert!(!blocked(ip), "expected {ip} to be allowed");
        }
    }
}
