//! Which clients may connect: a list of networks (`192.168.0.0/16`, `10.1.2.3`, `::1`).

use std::net::IpAddr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cidr {
    addr: u128,
    mask: u128,
    v6: bool,
}

fn bits(ip: IpAddr) -> (u128, bool) {
    match ip {
        IpAddr::V4(a) => (u128::from(u32::from(a)), false),
        IpAddr::V6(a) => match a.to_ipv4_mapped() {
            Some(v4) => (u128::from(u32::from(v4)), false),
            None => (u128::from(a), true),
        },
    }
}

impl Cidr {
    pub fn parse(s: &str) -> Result<Cidr, String> {
        let s = s.trim();
        let (ip, prefix) = match s.split_once('/') {
            Some((ip, p)) => (ip.trim(), Some(p.trim().parse::<u32>().map_err(|_| format!("{s:?} isn't a network: the part after / is a number"))?)),
            None => (s, None),
        };
        let ip: IpAddr = ip.parse().map_err(|_| format!("{s:?} isn't a network: write it as 192.168.0.0/16 or one address"))?;
        let (addr, v6) = bits(ip);
        let max = if v6 { 128 } else { 32 };
        let prefix = prefix.unwrap_or(max);
        if prefix > max {
            return Err(format!("{s:?} isn't a network: the most is /{max}"));
        }
        let host = max - prefix;
        let mask = if prefix == 0 { 0 } else { (!0u128 << host) & if v6 { !0u128 } else { u128::from(u32::MAX) } };
        Ok(Cidr { addr: addr & mask, mask, v6 })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        let (a, v6) = bits(ip);
        v6 == self.v6 && a & self.mask == self.addr
    }
}

/// A list of networks separated by commas, spaces or lines.
pub fn parse_list(s: &str) -> Result<Vec<Cidr>, String> {
    s.split(|c: char| c == ',' || c.is_whitespace()).filter(|p| !p.is_empty()).map(Cidr::parse).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn networks() {
        let l = parse_list("172.16.0.0/12, 192.168.0.0/16\n127.0.0.0/8").unwrap();
        let ok = |s: &str| l.iter().any(|c| c.contains(ip(s)));
        assert!(ok("172.17.0.5") && ok("172.16.0.1") && ok("172.31.255.255") && ok("192.168.1.20") && ok("127.0.0.1"));
        assert!(!ok("172.32.0.1") && !ok("172.15.255.255") && !ok("192.169.0.1") && !ok("10.0.0.1") && !ok("8.8.8.8"));
        assert!(ok("::ffff:192.168.1.20"), "an IPv4 client on an IPv6 socket");
        assert!(!ok("::1"));
        assert!(parse_list("::1/128").unwrap()[0].contains(ip("::1")));
        assert!(Cidr::parse("10.1.2.3").unwrap().contains(ip("10.1.2.3")) && !Cidr::parse("10.1.2.3").unwrap().contains(ip("10.1.2.4")));
        assert!(Cidr::parse("0.0.0.0/0").unwrap().contains(ip("8.8.8.8")));
        assert!(Cidr::parse("10.0.0.0/33").is_err() && Cidr::parse("banana").is_err() && Cidr::parse("10.0.0.0/x").is_err());
        assert!(parse_list("").unwrap().is_empty());
    }
}
