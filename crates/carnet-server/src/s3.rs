//! Pictures in an S3-compatible bucket (docs/SYNC.md §6, §11): AWS S3,
//! Cloudflare R2, Hetzner's, MinIO and the like. The `s3` feature builds
//! it. A device puts and fetches a picture straight to and from the
//! bucket, by a link pre-signed here: the bytes never pass through the
//! server. The server's own calls (is it there, how big, mark, sweep) are
//! few and small, made from the picture worker's thread, one at a time.
//!
//! ```text
//! <prefix><account>/<id>        a picture, sealed (account and id in hex)
//! <prefix>drop/<account>/<id>   a picture no note shows: when it was said, in seconds
//! <prefix>served                a server has run on this bucket (`migrate` won't copy over it)
//! ```
//!
//! **A put's size.** A pre-signed `PUT` can't say "at most so many bytes",
//! only an exact length: the device says how long each picture is sealed
//! when it offers it, and its link is signed with that `Content-Length`,
//! so the bucket refuses a put of any other length. The link is for one
//! picture, one account, five minutes, so a picture is never put past
//! what it was offered as, nor past [`MAX`]. (One put from before links
//! were signed so, past [`MAX`], is caught on the way back: [`Blobs::size`]
//! and [`Blobs::held`] find it too big, delete it, and it's never said
//! kept, linked, or counted.)
//!
//! **A put again.** Within its five minutes a link may put its picture
//! again, over the first. That's harmless, so it isn't stopped (a
//! conditional put, `If-None-Match`, would need every device to send the
//! header and every bucket's CORS to let it through): only the account's
//! own device was given the link, it can't put more bytes than it said,
//! and a picture that isn't the one its id names doesn't open on any
//! device (`open_blob`), as with any picture the account's device puts.
//! Nor is a sweep needed for pictures put and never named in a note: each
//! is counted against its account, and goes when a device drops it (§4).
//!
//! The bucket needs CORS for the web app: `PUT` and `GET` from the
//! server's origin (the README says how).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use md5::{Digest, Md5};
use rusty_s3::actions::{ListObjectsV2, ObjectIdentifier};
use rusty_s3::{Bucket, Credentials, S3Action, UrlStyle};

use crate::blobs::{Blobs, MAX};

/// How long a call the server makes itself is good for: signed, then
/// sent at once.
const CALL: Duration = Duration::from_secs(60);

/// Where a bucket is, and the keys to it.
#[derive(Clone)]
pub struct S3Config {
    /// `https://host[:port]/bucket[/prefix]`: the endpoint, the bucket
    /// (the path's first part), and a folder in it to keep to.
    pub url: String,
    /// `us-east-1` unless the host has its own (R2: `auto`).
    pub region: String,
    pub key: String,
    pub secret: String,
    /// The bucket as the host's first name (`bucket.host`), as AWS likes
    /// for new buckets; otherwise in the path, as everyone takes.
    pub virtual_host: bool,
}

impl std::fmt::Debug for S3Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Config").field("url", &self.url).field("region", &self.region).field("key", &self.key).finish_non_exhaustive()
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    (s.len().is_multiple_of(2) && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect())?
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Standard base64, for the MD5 S3 checks a put's body by.
fn base64(b: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in b.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A picture's MD5, in hex: what S3 says as a plain put's ETag.
pub fn md5_hex(b: &[u8]) -> String {
    hex(&Md5::digest(b))
}

/// A picture by whose it is: the account, the id.
pub type Picture = (Vec<u8>, [u8; 32]);

/// What an object is, as S3 says: its size, and its ETag unquoted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Object {
    pub key: String,
    pub size: u64,
    pub etag: String,
}

#[derive(Clone)]
pub struct S3Blobs {
    bucket: Bucket,
    creds: Credentials,
    prefix: String,
    agent: ureq::Agent,
}

impl S3Blobs {
    pub fn new(config: &S3Config) -> Result<Self, String> {
        let url = url::Url::parse(&config.url).map_err(|e| format!("{}: {e}", config.url))?;
        let mut parts = url.path().trim_matches('/').splitn(2, '/');
        let name = parts.next().filter(|n| !n.is_empty()).ok_or_else(|| format!("{}: no bucket in it (https://host/bucket)", config.url))?.to_string();
        let prefix = parts.next().map(|p| format!("{}/", p.trim_matches('/'))).filter(|p| p != "/").unwrap_or_default();
        let mut endpoint = url.clone();
        endpoint.set_path("/");
        endpoint.set_query(None);
        let style = if config.virtual_host { UrlStyle::VirtualHost } else { UrlStyle::Path };
        let bucket = Bucket::new(endpoint, style, name, config.region.clone()).map_err(|e| format!("{}: {e}", config.url))?;
        let agent = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .http_status_as_error(false)
                .timeout_connect(Some(Duration::from_secs(10)))
                .timeout_global(Some(Duration::from_secs(300)))
                .build(),
        );
        Ok(S3Blobs { bucket, creds: Credentials::new(config.key.clone(), config.secret.clone()), prefix, agent })
    }

    /// Where `account`'s picture `id` is.
    pub fn key(&self, account: &[u8], id: &[u8; 32]) -> String {
        format!("{}{}/{}", self.prefix, hex(account), hex(id))
    }

    fn mark_key(&self, account: &[u8], id: &[u8; 32]) -> String {
        format!("{}drop/{}/{}", self.prefix, hex(account), hex(id))
    }

    /// The bucket there, and the keys good for it.
    pub fn check(&self) -> Result<(), String> {
        let url = self.bucket.head_bucket(Some(&self.creds)).sign(CALL);
        match self.agent.head(url.as_str()).call() {
            Ok(r) if r.status().is_success() => Ok(()),
            Ok(r) => Err(format!("bucket {}: {}", self.bucket.name(), r.status())),
            Err(e) => Err(format!("bucket {}: {e}", self.bucket.name())),
        }
    }

    /// The bucket made, if it isn't there: the tests' (a real one is made
    /// with its host's own tools, CORS and all).
    pub fn make_bucket(&self) -> Result<(), String> {
        if self.check().is_ok() {
            return Ok(());
        }
        let url = self.bucket.create_bucket(&self.creds).sign(CALL);
        let r = self.agent.put(url.as_str()).send(&[][..]).map_err(|e| format!("bucket {}: {e}", self.bucket.name()))?;
        if r.status().is_success() { Ok(()) } else { Err(format!("bucket {}: {}", self.bucket.name(), r.status())) }
    }

    /// What's at `key`, if anything.
    pub fn head(&self, key: &str) -> Result<Option<Object>, String> {
        let url = self.bucket.head_object(Some(&self.creds), key).sign(CALL);
        let r = self.agent.head(url.as_str()).call().map_err(|e| format!("{key}: {e}"))?;
        match r.status().as_u16() {
            404 => Ok(None),
            s if (200..300).contains(&s) => {
                let header = |n: &str| r.headers().get(n).and_then(|v| v.to_str().ok()).map(str::to_string);
                let size = header("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
                Ok(Some(Object { key: key.to_string(), size, etag: header("etag").unwrap_or_default().trim_matches('"').to_string() }))
            }
            s => Err(format!("{key}: {s}")),
        }
    }

    /// `bytes` kept at `key`, checked by S3 against their MD5 as they
    /// come.
    pub fn put(&self, key: &str, bytes: &[u8]) -> Result<(), String> {
        let url = self.bucket.put_object(Some(&self.creds), key).sign(CALL);
        let md5 = base64(&Md5::digest(bytes));
        let r = self
            .agent
            .put(url.as_str())
            .header("Content-MD5", &md5)
            .header("Content-Type", "application/octet-stream")
            .send(bytes)
            .map_err(|e| format!("{key}: {e}"))?;
        if r.status().is_success() { Ok(()) } else { Err(format!("{key}: {}", r.status())) }
    }

    /// What's at `key`, whole; `None` if nothing is.
    pub fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        let url = self.bucket.get_object(Some(&self.creds), key).sign(CALL);
        let mut r = self.agent.get(url.as_str()).call().map_err(|e| format!("{key}: {e}"))?;
        match r.status().as_u16() {
            404 => Ok(None),
            s if (200..300).contains(&s) => r.body_mut().with_config().limit(MAX + 1024).read_to_vec().map(Some).map_err(|e| format!("{key}: {e}")),
            s => Err(format!("{key}: {s}")),
        }
    }

    /// Nothing at `key`, whether there was or not.
    pub fn delete(&self, key: &str) -> Result<(), String> {
        let url = self.bucket.delete_object(Some(&self.creds), key).sign(CALL);
        let r = self.agent.delete(url.as_str()).call().map_err(|e| format!("{key}: {e}"))?;
        if r.status().is_success() || r.status().as_u16() == 404 { Ok(()) } else { Err(format!("{key}: {}", r.status())) }
    }

    /// Every object whose key starts with `prefix`, in the bucket's order.
    pub fn list(&self, prefix: &str) -> Result<Vec<Object>, String> {
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut action = self.bucket.list_objects_v2(Some(&self.creds));
            action.with_prefix(prefix);
            if let Some(t) = &token {
                action.with_continuation_token(t.clone());
            }
            let url = action.sign(CALL);
            let mut r = self.agent.get(url.as_str()).call().map_err(|e| format!("listing {prefix}: {e}"))?;
            let body = r.body_mut().with_config().limit(16 << 20).read_to_string().map_err(|e| format!("listing {prefix}: {e}"))?;
            if !r.status().is_success() {
                return Err(format!("listing {prefix}: {}", r.status()));
            }
            let page = ListObjectsV2::parse_response(&body).map_err(|e| format!("listing {prefix}: {e}"))?;
            out.extend(page.contents.into_iter().map(|c| Object { key: c.key, size: c.size, etag: c.etag.trim_matches('"').to_string() }));
            match page.next_continuation_token {
                Some(t) if !t.is_empty() => token = Some(t),
                _ => return Ok(out),
            }
        }
    }

    /// Every one of `keys` gone, a thousand to a call.
    pub fn delete_all(&self, keys: &[String]) -> Result<(), String> {
        for chunk in keys.chunks(1000) {
            let ids: Vec<ObjectIdentifier> = chunk.iter().map(|k| ObjectIdentifier::new(k.clone())).collect();
            let mut action = self.bucket.delete_objects(Some(&self.creds), ids.iter());
            action.set_quiet(true);
            let url = action.sign(CALL);
            let (body, md5) = action.body_with_md5();
            let r = self.agent.post(url.as_str()).header("Content-MD5", &md5).send(body.as_bytes()).map_err(|e| format!("deleting: {e}"))?;
            if !r.status().is_success() {
                return Err(format!("deleting: {}", r.status()));
            }
        }
        Ok(())
    }

    /// Said once a server runs on it: from then on it holds what no copy
    /// has.
    pub fn serving(&self) -> Result<(), String> {
        self.put(&format!("{}served", self.prefix), now().to_string().as_bytes())
    }

    /// Whether a server has run on it.
    pub fn served(&self) -> Result<bool, String> {
        Ok(self.head(&format!("{}served", self.prefix))?.is_some())
    }

    /// The account's pictures: each id, its size and ETag.
    pub fn pictures(&self, account: &[u8]) -> Result<Vec<([u8; 32], Object)>, String> {
        let prefix = format!("{}{}/", self.prefix, hex(account));
        Ok(self.list(&prefix)?.into_iter().filter_map(|o| Some((<[u8; 32]>::try_from(unhex(o.key.strip_prefix(&prefix)?)?).ok()?, o))).collect())
    }

    /// Every picture in the bucket (its folder): account, id, and what
    /// S3 says of it. Marks and the like aren't among them.
    pub fn all_pictures(&self) -> Result<Vec<(Picture, Object)>, String> {
        Ok(self
            .list(&self.prefix)?
            .into_iter()
            .filter_map(|o| {
                let (account, id) = o.key.strip_prefix(&self.prefix)?.split_once('/')?;
                let (account, id) = (unhex(account)?, <[u8; 32]>::try_from(unhex(id)?).ok()?);
                Some(((account, id), o))
            })
            .collect())
    }

    /// Pictures marked to go: account, id, and when.
    pub fn marks(&self) -> Result<Vec<(Picture, u64)>, String> {
        let prefix = format!("{}drop/", self.prefix);
        let mut out = Vec::new();
        for o in self.list(&prefix)? {
            let Some((account, id)) = o.key.strip_prefix(&prefix).and_then(|rest| rest.split_once('/')) else { continue };
            let (Some(account), Some(id)) = (unhex(account), unhex(id).and_then(|i| <[u8; 32]>::try_from(i).ok())) else { continue };
            let at = self.get(&o.key)?.and_then(|b| String::from_utf8(b).ok()?.trim().parse().ok()).unwrap_or(0);
            out.push(((account, id), at));
        }
        Ok(out)
    }

    /// The mark itself put, as of `at`: what `migrate` copies.
    pub fn put_mark(&self, account: &[u8], id: &[u8; 32], at: u64) -> Result<(), String> {
        self.put(&self.mark_key(account, id), at.to_string().as_bytes())
    }

    /// The mark taken back.
    pub fn delete_mark(&self, account: &[u8], id: &[u8; 32]) -> Result<(), String> {
        self.delete(&self.mark_key(account, id))
    }

    /// When `id` was marked to go, if it is.
    pub fn marked(&self, account: &[u8], id: &[u8; 32]) -> Result<Option<u64>, String> {
        Ok(self.get(&self.mark_key(account, id))?.map(|b| String::from_utf8_lossy(&b).trim().parse().unwrap_or(0)))
    }

    /// A device's put of `size` bytes at `key`, to be signed: with its
    /// length, so the bucket takes no other.
    fn put_action<'a>(&'a self, key: &'a str, size: u64) -> rusty_s3::actions::PutObject<'a> {
        let mut put = self.bucket.put_object(Some(&self.creds), key);
        put.headers_mut().insert("content-length", size.to_string());
        put
    }

    fn said<T>(r: Result<T, String>) -> Option<T> {
        r.map_err(|e| eprintln!("carnet-server: pictures: {e}")).ok()
    }
}

impl Blobs for S3Blobs {
    fn origin(&self) -> Option<String> {
        self.bucket.object_url("x").ok().map(|u| u.origin().ascii_serialization())
    }

    fn size(&mut self, account: &[u8], id: &[u8; 32]) -> Option<u64> {
        let key = self.key(account, id);
        let o = Self::said(self.head(&key))??;
        if o.size > MAX {
            // Put past what a picture may be: not one.
            let _ = Self::said(self.delete(&key));
            return None;
        }
        Some(o.size)
    }

    fn link(&mut self, account: &[u8], id: &[u8; 32], put: Option<u64>, expires: u64) -> String {
        let key = self.key(account, id);
        let left = Duration::from_secs(expires.saturating_sub(now()).max(1));
        if let Some(n) = put {
            self.put_action(&key, n).sign(left).to_string()
        } else {
            // Kept by the device's browser no longer than the link is good,
            // and never taken for anything but bytes.
            let mut get = self.bucket.get_object(Some(&self.creds), &key);
            get.query_mut().insert("response-cache-control", format!("private, max-age={}", left.as_secs()));
            get.query_mut().insert("response-content-type", "application/octet-stream");
            get.sign(left).to_string()
        }
    }

    fn forget(&mut self, account: &[u8]) {
        for prefix in [format!("{}{}/", self.prefix, hex(account)), format!("{}drop/{}/", self.prefix, hex(account))] {
            if let Some(all) = Self::said(self.list(&prefix)) {
                let keys: Vec<String> = all.into_iter().map(|o| o.key).collect();
                let _ = Self::said(self.delete_all(&keys));
            }
        }
    }

    fn mark(&mut self, account: &[u8], id: &[u8; 32], now: u64) {
        let kept = Self::said(self.head(&self.key(account, id))).flatten().is_some();
        if kept && Self::said(self.head(&self.mark_key(account, id))).is_some_and(|m| m.is_none()) {
            let _ = Self::said(self.put_mark(account, id, now));
        }
    }

    fn unmark(&mut self, account: &[u8], id: &[u8; 32]) {
        let _ = Self::said(self.delete_mark(account, id));
    }

    fn sweep(&mut self, before: u64) -> u64 {
        let Some(marks) = Self::said(self.marks()) else { return 0 };
        let mut gone = 0;
        for ((account, id), at) in marks {
            if at < before && Self::said(self.delete(&self.key(&account, &id))).is_some() {
                let _ = Self::said(self.delete(&self.mark_key(&account, &id)));
                gone += 1;
            }
        }
        gone
    }

    fn held(&mut self, account: &[u8]) -> Vec<([u8; 32], u64)> {
        let all = Self::said(self.pictures(account)).unwrap_or_default();
        let (fit, over): (Vec<_>, Vec<_>) = all.into_iter().partition(|(_, o)| o.size <= MAX);
        // Put past what a picture may be: not one.
        if !over.is_empty() {
            let _ = Self::said(self.delete_all(&over.into_iter().map(|(_, o)| o.key).collect::<Vec<_>>()));
        }
        fit.into_iter().map(|(id, o)| (id, o.size)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where the web app fetches pictures from: the bucket's origin, its
    /// own host when it's the host's first name.
    #[test]
    fn the_origin_is_where_links_point() {
        let at = |url: &str, virtual_host: bool| {
            let config = S3Config { url: url.into(), region: "auto".into(), key: "k".into(), secret: "s".into(), virtual_host };
            S3Blobs::new(&config).unwrap().origin()
        };
        assert_eq!(at("https://s3.example:9000/pics/carnet", false).as_deref(), Some("https://s3.example:9000"));
        assert_eq!(at("https://s3.example/pics", true).as_deref(), Some("https://pics.s3.example"));
    }

    /// A link to put is signed with the picture's length: the bucket
    /// checks the `Content-Length` sent against it, and refuses another.
    /// A link to fetch isn't.
    #[test]
    fn a_put_link_is_signed_with_its_length() {
        let config = S3Config { url: "https://s3.example.com/pics".into(), region: "auto".into(), key: "k".into(), secret: "s".into(), virtual_host: false };
        let mut b = S3Blobs::new(&config).unwrap();
        let put = b.link(b"\x01", &[2; 32], Some(5036), now() + 300);
        assert!(put.contains("X-Amz-SignedHeaders=content-length%3Bhost&"), "{put}");
        let get = b.link(b"\x01", &[2; 32], None, now() + 300);
        assert!(get.contains("X-Amz-SignedHeaders=host&"), "{get}");
        // The same moment, another length: another signature.
        let at = jiff::Timestamp::from_second(1_760_000_000).unwrap();
        let sig = |n: u64| {
            let url = b.put_action(&b.key(b"\x01", &[2; 32]), n).sign_with_time(Duration::from_secs(300), &at);
            url.query_pairs().find(|(k, _)| k == "X-Amz-Signature").map(|(_, v)| v.into_owned()).unwrap()
        };
        assert_eq!(sig(5036), sig(5036));
        assert_ne!(sig(5036), sig(5037));
    }

    #[test]
    fn base64_as_s3_reads_it() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(&Md5::digest(b"")), "1B2M2Y8AsgTpgAmY7PhCfg==");
    }

    #[test]
    fn a_bucket_and_its_folder_from_the_url() {
        let config = |url: &str| S3Config { url: url.into(), region: "auto".into(), key: "k".into(), secret: "s".into(), virtual_host: false };
        let b = S3Blobs::new(&config("https://s3.example.com/pics")).unwrap();
        assert_eq!(b.key(b"\x01", &[2; 32]), format!("01/{}", "02".repeat(32)));
        let b = S3Blobs::new(&config("http://127.0.0.1:9000/pics/carnet/")).unwrap();
        assert_eq!(b.key(b"\x01", &[2; 32]), format!("carnet/01/{}", "02".repeat(32)));
        assert_eq!(b.mark_key(b"\x01", &[2; 32]), format!("carnet/drop/01/{}", "02".repeat(32)));
        let put = S3Blobs::new(&config("https://s3.example.com/pics")).unwrap().link(b"\x01", &[2; 32], Some(5036), now() + 300);
        assert!(put.starts_with("https://s3.example.com/pics/01/") && put.contains("X-Amz-Signature="), "{put}");
        assert!(S3Blobs::new(&config("https://s3.example.com/")).is_err(), "no bucket");
        let debug = format!("{:?}", config("https://s3.example.com/pics"));
        assert!(!debug.contains("secret: \"s\""), "the secret isn't shown: {debug}");
    }
}
