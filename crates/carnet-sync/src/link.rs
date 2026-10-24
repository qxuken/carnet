//! A file's link, as the server gives it (docs/SYNC.md §6): read, and
//! checked before anything is sent to it.
//!
//! The server says where a file is put or fetched: its own
//! `/v1/blobs/…`, or an object store's signed link, on a host of its own.
//! A server that isn't what it seems could say anywhere: a service on this
//! computer or its network (a database, a printer's page, a router's), or
//! a link that is two requests in one. So a link is followed only when it
//! is plainly the web's ([`fair`]): https, or http to the server's own
//! origin (one tried out on this computer); no address on this computer or
//! its network, unless it's the server's own host, on one itself; and
//! nothing in it but printable ASCII, so no space, CR or LF ends a request
//! line early.

use std::net::IpAddr;

/// Where a link points: secure or not, host (lowercase, an IPv6 address
/// without its brackets), port, and the path with its query, as sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Url {
    pub tls: bool,
    pub host: String,
    pub port: u16,
    pub path: String,
}

impl Url {
    /// `http://` or `https://`, a host (a name, an IPv4 address, or an
    /// IPv6 one in brackets) and maybe a port, then a path. No user in it,
    /// and nothing but printable ASCII; a fragment (`#…`) is never sent.
    pub fn parse(url: &str) -> Result<Url, String> {
        if !url.bytes().all(|b| b.is_ascii_graphic()) {
            return Err("a link with a space, a control character or a letter past ASCII in it".into());
        }
        // ASCII from here: any byte is a char's start.
        let scheme = |s: &str| url.get(..s.len()).is_some_and(|p| p.eq_ignore_ascii_case(s));
        let (tls, rest) = if scheme("https://") {
            (true, &url[8..])
        } else if scheme("http://") {
            (false, &url[7..])
        } else {
            return Err(format!("{url}: not an http(s) link"));
        };
        let (authority, tail) = rest.split_at(rest.find(['/', '?', '#']).unwrap_or(rest.len()));
        let path = tail.split('#').next().unwrap_or("");
        let path = if path.starts_with('/') { path.to_string() } else { format!("/{path}") };
        if authority.contains('@') {
            return Err(format!("{url}: a link with a user in it"));
        }
        let (host, port) = match authority.strip_prefix('[') {
            Some(r) => {
                let (h, after) = r.split_once(']').ok_or_else(|| format!("{url}: an address not closed"))?;
                h.parse::<std::net::Ipv6Addr>().map_err(|_| format!("{url}: not an address"))?;
                match after {
                    "" => (h, None),
                    p => (h, Some(p.strip_prefix(':').ok_or_else(|| format!("{url}: not a port"))?)),
                }
            }
            None => match authority.split_once(':') {
                Some((h, p)) => (h, Some(p)),
                None => (authority, None),
            },
        };
        if host.is_empty() || !(host.contains(':') || host.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._".contains(&b))) {
            return Err(format!("{url}: no host, or not one"));
        }
        let port = match port {
            None => if tls { 443 } else { 80 },
            Some(p) => p.parse::<u16>().ok().filter(|n| *n != 0 && p.bytes().all(|b| b.is_ascii_digit())).ok_or_else(|| format!("{url}: not a port"))?,
        };
        Ok(Url { tls, host: host.to_ascii_lowercase(), port, path })
    }

    /// The host as a request's `Host` names it: with its port unless it's
    /// the scheme's own, an IPv6 address in brackets.
    pub fn host_header(&self) -> String {
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        if (self.tls && self.port == 443) || (!self.tls && self.port == 80) { host } else { format!("{host}:{}", self.port) }
    }
}

/// Whether the link `url`, given by the server at `origin`, may be
/// followed: it reads ([`Url::parse`]); it's https, or the server itself
/// is reached by plain http (one tried out, or on a home network, with
/// its object store beside it the same way: nothing a link could lower);
/// and it's on no host [`inner`] unless that's the server's own host, and
/// the server's host is inner itself. Any port of it: an object store may
/// sit beside the server on one of its own. Another host on this computer
/// or its network, even for a server on one, never: a server isn't let
/// knock on every door around it. An object store's link on a public host
/// of its own is fine.
pub fn fair(origin: &str, url: &str) -> bool {
    let (Ok(link), Ok(server)) = (Url::parse(url), Url::parse(origin)) else { return false };
    (link.tls || !server.tls) && (!inner(&link.host) || (inner(&server.host) && same_host(&link.host, &server.host)))
}

/// A page the server at `origin` names to be shown (its signup page): as
/// written, if it reads as a link does ([`Url::parse`]: printable ASCII,
/// no user in it) and is `https`, or `http` when the server itself is.
/// Anything else (`javascript:`, a `data:` page, letters that turn the
/// line around) isn't shown at all.
pub fn shown_page(origin: &str, page: &str) -> Option<String> {
    let (link, server) = (Url::parse(page).ok()?, Url::parse(origin).ok()?);
    (link.tls || !server.tls).then(|| crate::session::server_words(page)).filter(|p| p == page)
}

/// Whether two hosts, as [`Url`] has them (lowercase), are one: a name's
/// trailing dot aside.
pub(crate) fn same_host(a: &str, b: &str) -> bool {
    a.trim_end_matches('.') == b.trim_end_matches('.')
}

/// Whether `host` is this computer, or on its network: `localhost` (and
/// any name under it), or an address that's loopback, private,
/// link-local, shared (CGNAT), unspecified or broadcast (and such an
/// IPv4 address written as IPv6, NAT64's among them). A name the DNS
/// can't give, its last part beginning with a digit (`127.1`,
/// `2130706433`, `0x7f.1`), some systems read as an address: counted as
/// one.
pub(crate) fn inner(host: &str) -> bool {
    let host = host.trim_end_matches('.');
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(ip) => inner_ip(ip),
        Err(_) => host.rsplit('.').next().is_some_and(|l| l.starts_with(|c: char| c.is_ascii_digit())),
    }
}

/// Whether an address is this computer's, or on its network: what
/// [`inner`] counts, for an address a name was looked up to.
pub(crate) fn inner_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let o = v.octets();
            v.is_loopback() || v.is_private() || v.is_link_local() || v.is_unspecified() || v.is_broadcast() || o[0] == 0 || (o[0] == 100 && o[1] & 0xc0 == 64)
        }
        IpAddr::V6(v) => {
            let s = v.segments();
            // NAT64 (`64:ff9b::/96`): the IPv4 address in its last 32
            // bits, reached through a gateway; the local-use prefix
            // (`64:ff9b:1::/48`) is a network's own, whatever it holds.
            let nat64 = s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] && {
                let o = v.octets();
                inner_ip(IpAddr::V4(std::net::Ipv4Addr::new(o[12], o[13], o[14], o[15])))
            };
            v.is_loopback()
                || v.is_unspecified()
                || s[0] & 0xfe00 == 0xfc00
                || s[0] & 0xffc0 == 0xfe80
                || s[..3] == [0x64, 0xff9b, 1]
                || nat64
                || v.to_ipv4().is_some_and(|v4| inner_ip(IpAddr::V4(v4)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_read_as_they_say() {
        let u = Url::parse("https://bucket.example/acct/id?X-Amz-Signature=ab%2Fcd&x=1").unwrap();
        assert_eq!((u.tls, u.host.as_str(), u.port, u.path.as_str()), (true, "bucket.example", 443, "/acct/id?X-Amz-Signature=ab%2Fcd&x=1"));
        let u = Url::parse("http://127.0.0.1:8787/v1/blobs/ab").unwrap();
        assert_eq!((u.tls, u.host.as_str(), u.port, u.host_header()), (false, "127.0.0.1", 8787, "127.0.0.1:8787".into()));
        let u = Url::parse("http://[::1]:9000/x").unwrap();
        assert_eq!((u.host.as_str(), u.port, u.host_header()), ("::1", 9000, "[::1]:9000".into()));
        let u = Url::parse("HTTPS://Notes.Example?q=1#frag").unwrap();
        assert_eq!((u.host.as_str(), u.path.as_str(), u.host_header()), ("notes.example", "/?q=1", "notes.example".into()));
        for bad in [
            "ftp://x/y",
            "https:///x",
            "https://:443/x",
            "https://x:0/y",
            "https://x:+80/y",
            "https://x:99999/y",
            "https://user:pw@x/y",
            "https://[::1/x",
            "https://[nope]/x",
            "https://x/a b",
            "https://x/a\r\nHost: evil",
            "https://x\r\n/y",
            "https://x/\u{e9}",
            "https://ex\u{e4}mple.com/",
            "https://x;y/z",
        ] {
            assert!(Url::parse(bad).is_err(), "{bad:?}");
        }
    }

    /// The server's own links, and an object store's on a public host,
    /// are followed; one to this computer or its network, plain http
    /// elsewhere, or one that reads two ways, never.
    #[test]
    fn only_fair_links_are_followed() {
        let public = "https://notes.example.com";
        assert!(fair(public, "https://notes.example.com/v1/blobs/t"));
        assert!(fair(public, "https://bucket.example/x"));
        assert!(fair(public, "https://s3.eu-west-1.amazonaws.com/b/x?X-Amz-Signature=a"));
        for bad in [
            "http://127.0.0.1:2375/x",
            "https://127.0.0.1:2375/x",
            "https://10.0.0.1/x",
            "https://192.168.1.1/x",
            "https://172.16.0.5/x",
            "https://169.254.169.254/latest/meta-data",
            "https://100.64.0.1/x",
            "https://0.0.0.0/x",
            "https://[::1]/x",
            "https://[fe80::1]/x",
            "https://[fd00::1]/x",
            "https://[::ffff:127.0.0.1]/x",
            "https://[64:ff9b::a00:1]/x",
            "https://[64:ff9b:1::5]/x",
            "https://localhost/x",
            "https://LOCALHOST./x",
            "https://db.localhost/x",
            "https://127.1/x",
            "https://2130706433/x",
            "https://0x7f.1/x",
            "https://bucket.example/x\r\nX-Evil: 1",
            "https://bucket.example/x y",
            "http://bucket.example/x",
            "http://notes.example.com/v1/blobs/t",
            "javascript:alert(1)",
            "",
        ] {
            assert!(!fair(public, bad), "{bad:?}");
        }
        // A server tried out on this computer, by plain http: its own
        // links, and an object store beside it on another port, as nothing
        // is lowered by them. A name (a container's) is the DNS's to say,
        // and checked there (`http`).
        let here = "http://127.0.0.1:8787";
        assert!(fair(here, "http://127.0.0.1:8787/v1/blobs/t"));
        assert!(fair(here, "http://127.0.0.1:9000/bucket/x"));
        assert!(fair(here, "http://rustfs:9000/carnet/x"));
        assert!(fair(here, "https://bucket.example/x"));
        assert!(fair("http://localhost:8787", "http://localhost.:9000/bucket/x"));
        // But no other host on this computer or its network: not another
        // name for it, not the router, not a service on another address.
        assert!(!fair(here, "http://localhost:9000/bucket/x"));
        assert!(!fair(here, "http://127.0.0.2:2375/x"));
        assert!(!fair(here, "http://192.168.1.1/x"));
        assert!(!fair("http://localhost:8787", "http://127.0.0.1:2375/x"));
        // One on the internet by plain http: still nothing on this
        // computer or its network, a hostile one's links to a local
        // service (Docker's, here) above all.
        assert!(fair("http://notes.example.com", "http://bucket.example/x"));
        assert!(!fair("http://notes.example.com", "http://127.0.0.1:2375/x"));
        assert!(!fair("http://notes.evil", "http://127.0.0.1:2375/containers/json"));
        assert!(!fair("http://notes.evil", "http://[::1]:2375/x"));
        assert!(!fair("http://notes.evil", "http://[64:ff9b::7f00:1]:2375/x"));
        assert!(!fair("http://203.0.113.9:8787", "http://127.0.0.1:2375/x"));
        // On a home network: its own host, on any port. Known by a name,
        // the server isn't known to be on one.
        assert!(fair("https://192.168.1.5", "https://192.168.1.5/v1/blobs/t"));
        assert!(fair("https://192.168.1.5", "https://192.168.1.5:9000/b/x"));
        assert!(!fair("https://192.168.1.5", "https://192.168.1.6:9000/b/x"));
        assert!(!fair("https://nas.home.arpa", "https://192.168.1.6:9000/b/x"));
        // An origin that doesn't read: nothing followed.
        assert!(!fair("not a server", "https://bucket.example/x"));
    }
}
