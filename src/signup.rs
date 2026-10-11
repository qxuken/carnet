//! Making an account from Settings → Sync, on a server that takes signups
//! (a hosted one): the server typed is asked whether it does, once typing
//! pauses; if it does, "New here?" offers an email field, the server mails
//! an invite there, and its code goes into the code field, as any code.
//! A server of one's own (no signups) shows none of it.

use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use carnet_sync::http::{Answer, ask};
use carnet_sync::signup;

/// How long the server's address stays as typed before it's asked about.
const ASK_AFTER: Duration = Duration::from_millis(500);

type Asked = Receiver<Result<Answer, String>>;

#[derive(Default)]
pub struct Signup {
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
    /// The server as typed (its origin), and since when.
    wanted: Option<(String, Instant)>,
    /// What a server said of signups: its origin, and its signup page if
    /// it takes them.
    known: Option<(String, Option<String>)>,
    /// A server being asked whether it takes signups.
    asking: Option<(String, Asked)>,
    /// "New here?" opened: the email field shows.
    pub open: bool,
    /// The email typed.
    pub email: String,
    /// An invite being asked for, to this email.
    sending: Option<(String, Asked)>,
    /// The invite was sent to this email.
    pub sent_to: Option<String>,
    /// What more the server said as it sent it.
    pub said: Option<String>,
    /// Why no invite was sent.
    pub error: Option<String>,
}

impl Signup {
    /// How the app is woken when a server answers.
    pub fn set_wake(&mut self, wake: impl Fn() + Send + Sync + 'static) {
        self.wake = Some(Arc::new(wake));
    }

    /// The server's address as it's typed now: another server, and what
    /// was said of the last one goes.
    pub fn want(&mut self, typed: &str) {
        let origin = signup::origin(typed);
        if origin.as_deref() == self.wanted.as_ref().map(|(o, _)| o.as_str()) {
            return;
        }
        self.wanted = origin.map(|o| (o, Instant::now()));
        self.open = false;
        self.sent_to = None;
        self.said = None;
        self.error = None;
        self.sending = None;
    }

    /// Answers taken, and the server asked about once typing has paused.
    /// True when an invite has just been sent: its code is to be typed.
    pub fn tick(&mut self) -> bool {
        if let Some((o, at)) = &self.wanted
            && at.elapsed() >= ASK_AFTER
            && self.known.as_ref().is_none_or(|(k, _)| k != o)
            && self.asking.as_ref().is_none_or(|(a, _)| a != o)
        {
            let rx = ask(signup::info_url(o), None, self.waker()).ok();
            self.asking = rx.map(|rx| (o.clone(), rx));
        }
        if let Some((o, rx)) = &self.asking
            && let Ok(answer) = rx.try_recv()
        {
            let page = answer.ok().as_ref().and_then(signup::takes_signups);
            self.known = Some((o.clone(), page));
            self.asking = None;
        }
        let Some((to, rx)) = &self.sending else { return false };
        let Ok(answer) = rx.try_recv() else { return false };
        let to = to.clone();
        self.sending = None;
        let page = self.offered().map(str::to_string);
        match answer.map_err(|why| format!("Couldn't reach the server: {why}")).and_then(|a| signup::answered(&a, page.as_deref())) {
            Ok(said) => {
                self.sent_to = Some(to);
                self.said = said;
                self.open = false;
                true
            }
            Err(why) => {
                self.error = Some(why);
                false
            }
        }
    }

    /// How long until the server typed is asked about, if it's to be.
    pub fn due(&self) -> Option<Duration> {
        let (o, at) = self.wanted.as_ref()?;
        let asked = self.known.as_ref().is_some_and(|(k, _)| k == o) || self.asking.as_ref().is_some_and(|(a, _)| a == o);
        (!asked).then(|| ASK_AFTER.saturating_sub(at.elapsed()))
    }

    /// The signup page of the server typed, if it takes signups.
    pub fn offered(&self) -> Option<&str> {
        let (o, _) = self.wanted.as_ref()?;
        self.known.as_ref().filter(|(k, _)| k == o)?.1.as_deref()
    }

    /// Whether the server typed has been asked and has answered.
    #[cfg(test)]
    pub fn answered(&self) -> bool {
        self.wanted.as_ref().is_some_and(|(o, _)| self.known.as_ref().is_some_and(|(k, _)| k == o))
    }

    pub fn sending(&self) -> bool {
        self.sending.is_some()
    }

    /// An invite asked for, to the email typed.
    pub fn send(&mut self) {
        self.error = None;
        let email = self.email.trim().to_string();
        let Some((origin, _)) = self.wanted.clone() else { return };
        if self.offered().is_none() || self.sending() {
            return;
        }
        if !email.contains('@') || email.contains(char::is_whitespace) {
            self.error = Some("That doesn't look like an email address".into());
            return;
        }
        match ask(signup::signup_url(&origin), Some(signup::body(&email)), self.waker()) {
            Ok(rx) => self.sending = Some((email, rx)),
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    fn waker(&self) -> impl Fn() + Send + 'static {
        let wake = self.wake.clone();
        move || {
            if let Some(w) = &wake {
                w();
            }
        }
    }
}
