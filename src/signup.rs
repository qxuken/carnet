//! Making an account from Settings → Sync, on a server that takes signups
//! (a hosted one, as its `/v1/info` says: [`crate::connect`] asked): the
//! server mails an invite to the email typed, and its code goes into the
//! code field, as any code.

use std::sync::Arc;
use std::sync::mpsc::Receiver;

use carnet_sync::http::{Answer, ask};
use carnet_sync::signup;

type Asked = Receiver<Result<Answer, String>>;

#[derive(Default)]
pub struct Signup {
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
    /// The email typed.
    pub email: String,
    /// An invite being asked for: to this email, and the server's signup
    /// page if it has one (for a server whose signup is only there).
    sending: Option<(String, Option<String>, Asked)>,
    /// The invite was sent to this email.
    pub sent_to: Option<String>,
    /// What the server answered: sent now (with whatever more it said),
    /// or mailed lately and not again.
    pub sent: Option<signup::Sent>,
    /// Why no invite was sent.
    pub error: Option<String>,
}

impl Signup {
    /// How the app is woken when a server answers.
    pub fn set_wake(&mut self, wake: impl Fn() + Send + Sync + 'static) {
        self.wake = Some(Arc::new(wake));
    }

    /// Another server, or none: what was said of an invite from the last
    /// one goes (the email typed stays).
    pub fn reset(&mut self) {
        self.sending = None;
        self.sent_to = None;
        self.sent = None;
        self.error = None;
    }

    /// The server's answer taken. True when an invite has just been sent:
    /// its code is to be typed.
    pub fn tick(&mut self) -> bool {
        let Some((to, page, rx)) = &self.sending else { return false };
        let Ok(answer) = rx.try_recv() else { return false };
        let (to, page) = (to.clone(), page.clone());
        self.sending = None;
        match answer.map_err(|why| format!("Couldn't reach the server: {why}")).and_then(|a| signup::answered(&a, page.as_deref())) {
            Ok(sent) => {
                self.sent_to = Some(to);
                self.sent = Some(sent);
                true
            }
            Err(why) => {
                self.error = Some(why);
                false
            }
        }
    }

    pub fn sending(&self) -> bool {
        self.sending.is_some()
    }

    /// An invite asked of the server at `origin` (`page`: its own signup
    /// page, if it has one), to the email typed.
    pub fn send(&mut self, origin: &str, page: Option<&str>) {
        self.error = None;
        if self.sending() {
            return;
        }
        let email = self.email.trim().to_string();
        if !email.contains('@') || email.contains(char::is_whitespace) {
            self.error = Some("That doesn't look like an email address".into());
            return;
        }
        let wake = self.wake.clone();
        let waker = move || {
            if let Some(w) = &wake {
                w();
            }
        };
        match ask(signup::signup_url(origin), Some(signup::body(&email)), waker) {
            Ok(rx) => self.sending = Some((email, page.map(str::to_string), rx)),
            Err(e) => self.error = Some(e.to_string()),
        }
    }
}
