//! Settings → Sync's way in, while sync is off, one step at a time: the
//! server first (asked what it is, on Continue), then how you're getting
//! in, from the ways it lists (docs/SYNC.md §14), then that way's form.
//! A server that takes only codes has nothing to choose between: the
//! second step is skipped, straight to the code.

use std::sync::Arc;
use std::sync::mpsc::Receiver;

use carnet_sync::http::{Answer, ask};
use carnet_sync::info::{self, Problem, ServerInfo};
use carnet_sync::signup;

type Asked = Receiver<Result<Answer, String>>;

/// Where in the way in Settings → Sync is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Where the notes are: the server's address.
    Server,
    /// How you're getting in, from the ways the server lists.
    Way,
    /// The way chosen: its form.
    Form,
}

#[derive(Default)]
pub struct Connect {
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
    /// A server being asked what it is: its origin, and its answer to come.
    checking: Option<(String, Asked)>,
    /// The server asked, and what it says of itself: past the first step.
    server: Option<(String, ServerInfo)>,
    /// Why the server typed can't be gone on with, as it was last asked.
    pub problem: Option<Problem>,
    /// A way in chosen: its form shows.
    chosen: bool,
}

impl Connect {
    /// How the app is woken when a server answers.
    pub fn set_wake(&mut self, wake: impl Fn() + Send + Sync + 'static) {
        self.wake = Some(Arc::new(wake));
    }

    /// The server typed asked what it is (Continue).
    pub fn check(&mut self, typed: &str) {
        self.problem = None;
        let Some(origin) = signup::origin(typed) else {
            self.problem = Some(Problem::Unreachable);
            return;
        };
        let wake = self.wake.clone();
        let waker = move || {
            if let Some(w) = &wake {
                w();
            }
        };
        match ask(info::url(&origin), None, waker) {
            Ok(rx) => self.checking = Some((origin, rx)),
            Err(_) => self.problem = Some(Problem::Unreachable),
        }
    }

    /// Asking put aside (Cancel, or the address typed again).
    pub fn cancel(&mut self) {
        self.checking = None;
    }

    pub fn checking(&self) -> bool {
        self.checking.is_some()
    }

    /// The server's answer taken. True when it's just been found fine:
    /// on to the second step, or, with nothing to choose, the third.
    pub fn tick(&mut self) -> bool {
        let Some((origin, rx)) = &self.checking else { return false };
        let Ok(answer) = rx.try_recv() else { return false };
        let origin = origin.clone();
        self.checking = None;
        match info::checked(answer) {
            Ok(info) => {
                self.found(origin, info);
                true
            }
            Err(p) => {
                self.problem = Some(p);
                false
            }
        }
    }

    fn found(&mut self, origin: String, info: ServerInfo) {
        self.problem = None;
        self.server = Some((origin, info));
        self.chosen = !self.choice();
    }

    /// A server taken as answered, without asking it.
    #[cfg(test)]
    pub fn assume(&mut self, origin: &str, info: ServerInfo) {
        self.found(origin.to_string(), info);
    }

    pub fn step(&self) -> Step {
        match (&self.server, self.chosen) {
            (None, _) => Step::Server,
            (Some(_), false) => Step::Way,
            (Some(_), true) => Step::Form,
        }
    }

    /// Whether the server lists more ways in than a code (the words are
    /// always there, asked for on purpose): if not, no second step.
    pub fn choice(&self) -> bool {
        self.info().is_some_and(|i| i.takes_passwords() || i.takes_signups())
    }

    /// The server asked, and found fine.
    pub fn origin(&self) -> Option<&str> {
        self.server.as_ref().map(|(o, _)| o.as_str())
    }

    pub fn info(&self) -> Option<&ServerInfo> {
        self.server.as_ref().map(|(_, i)| i)
    }

    /// A way in chosen: its form.
    pub fn choose(&mut self) {
        if self.server.is_some() {
            self.chosen = true;
        }
    }

    /// Back to the first step (Change), the address as typed.
    pub fn change(&mut self) {
        self.checking = None;
        self.server = None;
        self.problem = None;
        self.chosen = false;
    }

    /// One step back from the form: to the ways in, or, with none to
    /// choose between, to the server.
    pub fn back(&mut self) {
        if self.choice() {
            self.chosen = false;
        } else {
            self.change();
        }
    }
}
