//! How much one client may ask of the server (docs/SYNC.md §6), with no
//! I/O in it: token buckets by client, and which address a request is
//! from when a proxy stands in front.
//!
//! A client is an IPv4 address, or an IPv6 network's first 64 bits (a
//! /64): one household's, or one server's, is handed a whole /64, so
//! counting each IPv6 address apart would give one of them endless
//! buckets. Two things are limited: sockets opened (the
//! server, before the upgrade), and tries that cost the server work or
//! guess at something (the hub: a login that fails, an invite used, a
//! pairing joined, a recovery). A login that succeeds costs nothing, so a
//! device reconnecting never shuts itself out.

use std::collections::HashMap;
use std::net::IpAddr;
use std::str::FromStr;

/// So many at once, then one every so many seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rate {
    pub burst: f64,
    pub every: f64,
}

/// Sockets one client opens: thirty at once, then one every two seconds.
pub const SOCKETS: Rate = Rate { burst: 30.0, every: 2.0 };

/// Sockets one client holds open at once: a household's devices behind
/// one address, with room to spare.
pub const MAX_SOCKETS: usize = 64;

/// Tries one client makes: ten at once, then one every six seconds.
pub const TRIES: Rate = Rate { burst: 10.0, every: 6.0 };

/// What a client turned away for trying too much is told.
pub const TOO_MANY: &str = "too many tries from this address: wait a minute, then try again";

#[derive(Clone, Copy, Debug)]
struct Bucket {
    tokens: f64,
    at: f64,
}

/// A bucket for each client: [`Limiter::allow`] takes one try from it.
#[derive(Debug)]
pub struct Limiter {
    rate: Rate,
    buckets: HashMap<String, Bucket>,
}

impl Limiter {
    pub fn new(rate: Rate) -> Self {
        Limiter { rate, buckets: HashMap::new() }
    }

    /// One try by `client` at `now` (seconds, from any start): whether it
    /// may. An empty client is one the server can't tell (a test's): never
    /// limited.
    pub fn allow(&mut self, client: &str, now: f64) -> bool {
        if client.is_empty() {
            return true;
        }
        let Rate { burst, every } = self.rate;
        let b = self.buckets.entry(client.to_string()).or_insert(Bucket { tokens: burst, at: now });
        b.tokens = (b.tokens + (now - b.at).max(0.0) / every).min(burst);
        b.at = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Buckets full again forgotten: a client not seen for a while costs
    /// nothing to keep.
    pub fn prune(&mut self, now: f64) {
        let Rate { burst, every } = self.rate;
        self.buckets.retain(|_, b| b.tokens + (now - b.at).max(0.0) / every < burst);
    }

    pub fn len(&self) -> usize {
        self.buckets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buckets.is_empty()
    }
}

/// Addresses: one (`10.0.0.2`, `::1`) or a range (`10.0.0.0/8`,
/// `fd00::/8`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Network {
    addr: IpAddr,
    bits: u8,
}

impl FromStr for Network {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let s = s.trim();
        let (addr, bits) = s.split_once('/').map_or((s, None), |(a, b)| (a, Some(b)));
        let addr: IpAddr = addr.parse().map_err(|_| format!("{s}: not an address"))?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let bits = match bits {
            Some(b) => b.parse().ok().filter(|&b: &u8| b <= max).ok_or(format!("{s}: not a range"))?,
            None => max,
        };
        Ok(Network { addr: addr.to_canonical(), bits })
    }
}

impl Network {
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, ip.to_canonical()) {
            (IpAddr::V4(n), IpAddr::V4(a)) => prefix(u128::from(u32::from(n)), u128::from(u32::from(a)), self.bits, 32),
            (IpAddr::V6(n), IpAddr::V6(a)) => prefix(u128::from(n), u128::from(a), self.bits, 128),
            _ => false,
        }
    }
}

fn prefix(n: u128, a: u128, bits: u8, width: u8) -> bool {
    if bits == 0 {
        return true;
    }
    let shift = u32::from(width - bits);
    (n >> shift) == (a >> shift)
}

/// The proxies a list names (`10.0.0.2, 172.16.0.0/12`).
pub fn networks(list: &str) -> Result<Vec<Network>, String> {
    list.split(',').filter(|s| !s.trim().is_empty()).map(str::parse).collect()
}

/// Who a request is from, as the limits count it ([`client`]): the
/// address it came from, unless that is a proxy named as trusted, which
/// says who it came from for. Cloudflare's `CF-Connecting-IP` first; else
/// `X-Forwarded-For`, read from the right (the proxy's own addition) past
/// every trusted proxy. Anyone else's headers are ignored: they could say
/// anything.
pub fn client_of(peer: IpAddr, cf: Option<&str>, forwarded: Option<&str>, trusted: &[Network]) -> IpAddr {
    let trusted_ip = |ip: IpAddr| trusted.iter().any(|n| n.contains(ip));
    if !trusted_ip(peer) {
        return client(peer);
    }
    if let Some(ip) = cf.and_then(|c| c.trim().parse::<IpAddr>().ok()) {
        return client(ip);
    }
    let mut last = peer;
    for hop in forwarded.unwrap_or_default().rsplit(',') {
        let Ok(ip) = hop.trim().parse::<IpAddr>() else { break };
        if !trusted_ip(ip) {
            return client(ip);
        }
        last = ip;
    }
    client(last)
}

/// The client an address counts as: an IPv4 address itself (written as
/// IPv6 or not), an IPv6 address its /64, the rest zero.
pub fn client(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(v6) => IpAddr::V6((u128::from(v6) & !u128::from(u64::MAX)).into()),
        v4 => v4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bucket_lets_a_burst_through_then_one_at_a_time() {
        let mut l = Limiter::new(Rate { burst: 3.0, every: 10.0 });
        assert!((0..3).all(|_| l.allow("a", 0.0)));
        assert!(!l.allow("a", 0.0), "the burst spent");
        assert!(l.allow("b", 0.0), "another client has its own");
        assert!(!l.allow("a", 9.0));
        assert!(l.allow("a", 10.5), "one back after `every`");
        assert!(l.allow("", 0.0) && l.allow("", 0.0) && l.allow("", 0.0) && l.allow("", 0.0), "no client: never limited");
        l.prune(100.0);
        assert!(l.is_empty(), "full again: forgotten");
    }

    #[test]
    fn a_proxy_named_trusted_says_who_it_is_for_and_nobody_else_does() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        let trusted = networks("127.0.0.1, 10.0.0.0/8").unwrap();
        // Not a trusted proxy: its headers say nothing.
        assert_eq!(client_of(ip("203.0.113.9"), Some("1.2.3.4"), Some("5.6.7.8"), &trusted), ip("203.0.113.9"));
        // Cloudflare's header, from a trusted peer.
        assert_eq!(client_of(ip("127.0.0.1"), Some("198.51.100.7"), None, &trusted), ip("198.51.100.7"));
        // X-Forwarded-For from the right, past the trusted hops; what a
        // client wrote itself on the left is not believed.
        assert_eq!(client_of(ip("127.0.0.1"), None, Some("6.6.6.6, 198.51.100.7, 10.1.2.3"), &trusted), ip("198.51.100.7"));
        assert_eq!(client_of(ip("127.0.0.1"), None, None, &trusted), ip("127.0.0.1"), "no header: the proxy itself");
        assert_eq!(client_of(ip("::ffff:127.0.0.1"), Some("198.51.100.7"), None, &trusted), ip("198.51.100.7"), "a v4 peer as v6");
        assert!(networks("10.0.0.0/33").is_err() && networks("nope").is_err());
        assert!("::1".parse::<Network>().unwrap().contains(ip("::1")));
        assert!("0.0.0.0/0".parse::<Network>().unwrap().contains(ip("8.8.8.8")));
    }

    #[test]
    fn an_ipv6_client_is_its_64_and_an_ipv4_one_its_address() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        let none: &[Network] = &[];
        let of = |s: &str| client_of(ip(s), None, None, none).to_string();
        // One /64, any address in it: one client, one bucket.
        assert_eq!(of("2001:db8:1:2::1"), "2001:db8:1:2::");
        assert_eq!(of("2001:db8:1:2:ffff:ffff:ffff:ffff"), of("2001:db8:1:2::1"));
        assert_ne!(of("2001:db8:1:3::1"), of("2001:db8:1:2::1"), "the next /64 is someone else");
        // IPv4, as itself or mapped into IPv6: the whole address.
        assert_eq!(of("198.51.100.7"), "198.51.100.7");
        assert_eq!(of("::ffff:198.51.100.7"), "198.51.100.7");
        assert_ne!(of("198.51.100.8"), of("198.51.100.7"));
        // What a trusted proxy says is grouped the same.
        let trusted = networks("127.0.0.1").unwrap();
        assert_eq!(client_of(ip("127.0.0.1"), Some("2001:db8::abcd"), None, &trusted).to_string(), "2001:db8::");
        assert_eq!(client_of(ip("127.0.0.1"), None, Some("2001:db8:0:0:1::9"), &trusted).to_string(), "2001:db8::");

        // So a /64's addresses share the limits: its 31st socket at once,
        // and its 11th try, are refused whichever address asks.
        let mut sockets = Limiter::new(SOCKETS);
        let opened = (0..40u16).filter(|n| sockets.allow(&of(&format!("2001:db8:1:2::{n:x}")), 0.0)).count();
        assert_eq!(opened, SOCKETS.burst as usize);
        let mut tries = Limiter::new(TRIES);
        let tried = (0..40u16).filter(|n| tries.allow(&of(&format!("2001:db8:1:2:{n:x}::1")), 0.0)).count();
        assert_eq!(tried, TRIES.burst as usize);
    }
}
