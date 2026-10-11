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
/// and `https://` unless it says otherwise. `None` for nothing typed.
pub fn origin(typed: &str) -> Option<String> {
    let o = typed.trim().trim_end_matches('/');
    if o.is_empty() || o.contains(char::is_whitespace) {
        return None;
    }
    Some(if o.contains("://") { o.to_string() } else { format!("https://{o}") })
}

/// Where a server says what it is.
pub fn info_url(origin: &str) -> String {
    format!("{origin}/v1/info")
}

/// Where a server takes a signup.
pub fn signup_url(origin: &str) -> String {
    format!("{origin}/v1/signup")
}

/// The signup asked for: the email, as JSON.
pub fn body(email: &str) -> Vec<u8> {
    serde_json::json!({ "email": email.trim() }).to_string().into_bytes()
}

/// The server's signup page, if its `/v1/info` says it takes signups.
pub fn takes_signups(info: &Answer) -> Option<String> {
    if !(200..300).contains(&info.status) {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&info.body).ok()?;
    v.get("signup")?.as_str().filter(|s| !s.trim().is_empty()).map(str::to_string)
}

/// What an answer to a signup means: sent (with whatever more the server
/// said of it), or why not, in the server's words when it gives any.
/// `page` is the server's signup page, for a server whose signup is only
/// there.
pub fn answered(a: &Answer, page: Option<&str>) -> Result<Option<String>, String> {
    let said = said(&a.body);
    if (200..300).contains(&a.status) {
        // A plain "sent": nothing more to say.
        return Ok(said.filter(|_| a.status != 204));
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
        assert_eq!(origin("  "), None);
        assert_eq!(origin("not an address"), None);
        assert_eq!(signup_url("https://x.co"), "https://x.co/v1/signup");
        assert_eq!(body(" ann@example.com "), br#"{"email":"ann@example.com"}"#);
    }

    #[test]
    fn a_server_says_whether_it_takes_signups() {
        assert_eq!(takes_signups(&answer(200, r#"{"server":"carnet-server","signup":"https://x.co/start"}"#)).as_deref(), Some("https://x.co/start"));
        assert_eq!(takes_signups(&answer(200, r#"{"server":"carnet-server","passwords":false}"#)), None, "a server of one's own");
        assert_eq!(takes_signups(&answer(404, r#"{"signup":"https://x.co/start"}"#)), None);
        assert_eq!(takes_signups(&answer(200, "<html>")), None);
    }

    #[test]
    fn an_answer_in_the_servers_words() {
        assert_eq!(answered(&answer(202, r#"{"sent":true}"#), None), Ok(None));
        assert_eq!(answered(&answer(202, r#"{"sent":true,"message":"Mailed already: look for it."}"#), None), Ok(Some("Mailed already: look for it.".into())));
        assert_eq!(answered(&answer(400, r#"{"error":"That doesn't look like an email address."}"#), None), Err("That doesn't look like an email address.".into()));
        assert_eq!(answered(&answer(429, "Mailed lately: wait ten minutes.\n"), None), Err("Mailed lately: wait ten minutes.".into()));
        assert_eq!(answered(&answer(429, ""), None), Err("Too many tries: wait a minute, then try again.".into()));
        assert_eq!(answered(&answer(503, "<html>oops</html>"), None), Err("Signing up isn't working just now: try again in a few minutes.".into()));
        assert_eq!(answered(&answer(404, ""), Some("https://x.co/start")), Err("This server makes accounts on its own page: https://x.co/start".into()));
        assert_eq!(answered(&answer(403, ""), None), Err("The server didn't take that (error 403).".into()));
    }
}
