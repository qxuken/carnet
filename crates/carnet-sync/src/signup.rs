//! Making an account from the app, on a server that takes signups (a
//! hosted service, docs/SYNC.md §11): `/v1/info` says so with a link to
//! its signup page, and its `POST /v1/signup` takes `{"email": …}` and
//! mails an invite there. The app asks for one in place, and the invite's
//! code goes into the field codes always go into.
//!
//! What's asked, and what an answer means, here; the asking is
//! [`http::ask`](crate::http::ask)'s.

use crate::http::Answer;

/// A server's address as typed, as the app uses it: no slash at its end,
/// and `https://` unless it says otherwise, or `http://` for this
/// computer (`localhost`, `127.0.0.1`, `[::1]`), where a server of one's
/// own being tried out has no certificate. `None` for nothing typed.
pub fn origin(typed: &str) -> Option<String> {
    let o = typed.trim().trim_end_matches('/');
    if o.is_empty() || o.contains(char::is_whitespace) {
        return None;
    }
    if o.contains("://") {
        return Some(o.to_string());
    }
    let host = match o.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => o.split([':', '/']).next().unwrap_or(""),
    };
    let here = host.eq_ignore_ascii_case("localhost") || host == "::1" || host.starts_with("127.");
    Some(format!("{}://{o}", if here { "http" } else { "https" }))
}

/// Where a server takes a signup.
pub fn signup_url(origin: &str) -> String {
    format!("{origin}/v1/signup")
}

/// The signup asked for: the email, as JSON.
pub fn body(email: &str) -> Vec<u8> {
    serde_json::json!({ "email": email.trim() }).to_string().into_bytes()
}

/// An invite asked for, and answered yes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sent {
    /// Mailed now, with whatever more the server said of it.
    Now(Option<String>),
    /// Nothing mailed: one went to this address `ago` seconds since, and
    /// another may be asked for in `again_in` (a hosted service's
    /// `{"sent": false, "mailed_ago", "again_in"}`, as its own signup page
    /// says it).
    Lately { ago: u64, again_in: u64 },
}

impl Sent {
    /// What's said of it, to `to`: where the code goes; asked again too
    /// soon, when the last one went, and when another may be asked for.
    pub fn line(&self, to: &str) -> String {
        match self {
            Sent::Now(_) => format!("We sent an invite to {to}. Paste its code here."),
            Sent::Lately { ago, again_in } => {
                let ago = if *ago < 60 { "a moment ago".to_string() } else { format!("{} ago", minutes((ago + 30) / 60)) };
                format!(
                    "We emailed {to} {ago}. Look for it in your inbox (and the spam folder), and paste its code here. You can ask for another email in {}.",
                    minutes(again_in.div_ceil(60).max(1))
                )
            }
        }
    }

    /// More the server said as it sent it.
    pub fn more(&self) -> Option<&str> {
        match self {
            Sent::Now(more) => more.as_deref(),
            Sent::Lately { .. } => None,
        }
    }

    /// Asked again too soon: nothing new was mailed.
    pub fn lately(&self) -> bool {
        matches!(self, Sent::Lately { .. })
    }
}

fn minutes(n: u64) -> String {
    if n == 1 { "1 minute".into() } else { format!("{n} minutes") }
}

/// What an answer to a signup means: sent (with whatever more the server
/// said of it), mailed lately and not again, or why not, in the server's
/// words when it gives any. `page` is the server's signup page, for a
/// server whose signup is only there.
pub fn answered(a: &Answer, page: Option<&str>) -> Result<Sent, String> {
    let said = said(&a.body);
    if (200..300).contains(&a.status) {
        // Asked again within the server's mail limit: nothing went.
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&a.body)
            && v.get("sent").and_then(serde_json::Value::as_bool) == Some(false)
        {
            let n = |k: &str| v.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0);
            return Ok(Sent::Lately { ago: n("mailed_ago"), again_in: n("again_in") });
        }
        // A plain "sent": nothing more to say.
        return Ok(Sent::Now(said.filter(|_| a.status != 204)));
    }
    if matches!(a.status, 404 | 405)
        && let Some(page) = page
    {
        return Err(format!("This server makes accounts on its own page: {page}"));
    }
    Err(said.unwrap_or_else(|| match a.status {
        429 => "Too many tries: wait a minute, then try again.".into(),
        400..=499 => format!("The server didn't take that (error {}).", a.status),
        _ => "Signing up isn't working just now: try again in a few minutes.".into(),
    }))
}

/// The words in an answer: its JSON's `error` or `message`, or its plain
/// text if it's short (not a page).
fn said(body: &[u8]) -> Option<String> {
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) {
        return ["error", "message"].iter().find_map(|k| v.get(k)?.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    }
    let text = std::str::from_utf8(body).ok()?.trim();
    (!text.is_empty() && text.len() <= 300 && !text.starts_with('<')).then(|| text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(status: u16, body: &str) -> Answer {
        Answer { status, body: body.as_bytes().to_vec() }
    }

    #[test]
    fn a_servers_address_as_typed() {
        assert_eq!(origin(" notes.example.com/ ").as_deref(), Some("https://notes.example.com"));
        assert_eq!(origin("http://127.0.0.1:8787").as_deref(), Some("http://127.0.0.1:8787"));
        // This computer: plain http, as a server tried out here serves.
        assert_eq!(origin("localhost:8880").as_deref(), Some("http://localhost:8880"));
        assert_eq!(origin("127.0.0.1:8787/").as_deref(), Some("http://127.0.0.1:8787"));
        assert_eq!(origin("[::1]:8080").as_deref(), Some("http://[::1]:8080"));
        assert_eq!(origin("localhost.example.com").as_deref(), Some("https://localhost.example.com"));
        assert_eq!(origin("https://localhost:8443").as_deref(), Some("https://localhost:8443"));
        assert_eq!(origin("  "), None);
        assert_eq!(origin("not an address"), None);
        assert_eq!(signup_url("https://x.co"), "https://x.co/v1/signup");
        assert_eq!(body(" ann@example.com "), br#"{"email":"ann@example.com"}"#);
    }

    #[test]
    fn an_answer_in_the_servers_words() {
        assert_eq!(answered(&answer(202, r#"{"sent":true}"#), None), Ok(Sent::Now(None)));
        assert_eq!(answered(&answer(202, r#"{"sent":true,"message":"Mailed already: look for it."}"#), None), Ok(Sent::Now(Some("Mailed already: look for it.".into()))));
        assert_eq!(answered(&answer(400, r#"{"error":"That doesn't look like an email address."}"#), None), Err("That doesn't look like an email address.".into()));
        assert_eq!(answered(&answer(429, "Mailed lately: wait ten minutes.\n"), None), Err("Mailed lately: wait ten minutes.".into()));
        assert_eq!(answered(&answer(429, ""), None), Err("Too many tries: wait a minute, then try again.".into()));
        assert_eq!(answered(&answer(503, "<html>oops</html>"), None), Err("Signing up isn't working just now: try again in a few minutes.".into()));
        assert_eq!(answered(&answer(404, ""), Some("https://x.co/start")), Err("This server makes accounts on its own page: https://x.co/start".into()));
        assert_eq!(answered(&answer(403, ""), None), Err("The server didn't take that (error 403).".into()));
    }

    /// Asked again within the server's mail limit: nothing was mailed, so
    /// not "sent", but when the last one went, and when another may be
    /// asked for.
    #[test]
    fn asked_again_too_soon_says_when_it_went() {
        let a = answered(&answer(202, r#"{"sent":false,"mailed_ago":12,"again_in":588}"#), None).unwrap();
        assert_eq!(a, Sent::Lately { ago: 12, again_in: 588 });
        assert!(a.lately() && a.more().is_none());
        assert_eq!(
            a.line("ann@example.com"),
            "We emailed ann@example.com a moment ago. Look for it in your inbox (and the spam folder), and paste its code here. You can ask for another email in 10 minutes."
        );
        let a = answered(&answer(202, r#"{"sent":false,"mailed_ago":130,"again_in":30}"#), None).unwrap();
        let line = a.line("a@b.co");
        assert!(line.starts_with("We emailed a@b.co 2 minutes ago.") && line.ends_with("another email in 1 minute."), "{line}");
        let sent = answered(&answer(202, r#"{"sent":true}"#), None).unwrap();
        assert!(!sent.lately());
        assert_eq!(sent.line("ann@example.com"), "We sent an invite to ann@example.com. Paste its code here.");
    }
}
