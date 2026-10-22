//! The notes folder watched for what other apps do to it: a note written,
//! changed, renamed or deleted, a folder made, a picture put beside. The
//! system says when something in it changes (FSEvents on macOS, inotify
//! on Linux, ReadDirectoryChangesW on Windows, through `notify`), so a
//! folder left as it is costs nothing at all. A burst of changes (a sync
//! client writing 10,000 files) is taken as one, once it has settled.
//!
//! What the app itself wrote (a save, a note come from another device)
//! is left out: a file still as the store left it ([`store::is_own`]) is
//! no news. Its temporary files and hidden ones (`.trash`, `.pinned`) are
//! none either.
//!
//! Where the system won't watch the folder (a network share, too many
//! watches), the folder is looked at whole instead: every half minute, a
//! sum of its files' names, sizes and times. Where the system says it lost
//! track (its queue overflowed), the folder is caught up with once.

use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use notify::event::ModifyKind;
use notify::{EventKind, RecursiveMode, Watcher as _};

use crate::store;

/// How long changes must have stopped for a burst of them to be taken.
const SETTLE: Duration = Duration::from_millis(250);

/// How often the folder is looked at whole, where the system won't say.
pub const POLL_EVERY: Duration = if cfg!(test) { Duration::from_millis(100) } else { Duration::from_secs(30) };

pub struct Watch {
    dir: PathBuf,
    changed: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    /// The system's watch; `None` where it couldn't start, and the folder
    /// is looked at every [`POLL_EVERY`] instead. Held for its drop: its
    /// thread ends with it.
    #[cfg_attr(not(test), allow(dead_code))]
    events: Option<notify::RecommendedWatcher>,
}

/// What the system said.
enum Said {
    Paths(Vec<PathBuf>),
    /// It lost track (its queue overflowed) or failed: anything may have
    /// changed.
    Anything,
}

impl Watch {
    /// `dir` watched, `wake` called from another thread when something in
    /// it changed. The start counts as a change: what came between the
    /// folder's last reading and the watch.
    pub fn start(dir: PathBuf, wake: impl Fn() + Send + 'static) -> Self {
        Self::start_as(dir, wake, true)
    }

    fn start_as(dir: PathBuf, wake: impl Fn() + Send + 'static, events: bool) -> Self {
        let changed = Arc::new(AtomicBool::new(true));
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel::<Said>();
        let watcher = events
            .then(|| {
                let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                    let said = match res {
                        Ok(e) if e.need_rescan() => Said::Anything,
                        // Reads and opens are no change.
                        Ok(e) if matches!(e.kind, EventKind::Access(_)) => return,
                        // A folder's own times changing is no change
                        // either: Windows says so of a folder whenever
                        // something in it comes or goes (a save's
                        // temporary file), and what came or went is said
                        // of itself. A folder renamed still is.
                        Ok(e) if matches!(e.kind, EventKind::Modify(k) if !matches!(k, ModifyKind::Name(_)))
                            && e.paths.iter().all(|p| p.is_dir()) =>
                        {
                            return;
                        }
                        Ok(e) => Said::Paths(e.paths),
                        Err(_) => Said::Anything,
                    };
                    let _ = tx.send(said);
                })
                .ok()?;
                w.watch(&dir, RecursiveMode::Recursive).ok()?;
                Some(w)
            })
            .flatten();
        let (seen, stopped, at) = (changed.clone(), stop.clone(), dir.clone());
        let name = std::thread::Builder::new().name("carnet-watch".into());
        let _ = if watcher.is_some() {
            name.spawn(move || {
                let root = fs::canonicalize(&at).unwrap_or_else(|_| at.clone());
                // Ends when the watcher goes, and with it the sender.
                while let Ok(first) = rx.recv() {
                    let mut said = vec![first];
                    while let Ok(more) = rx.recv_timeout(SETTLE) {
                        said.push(more);
                    }
                    let news = said.iter().any(|s| match s {
                        Said::Anything => true,
                        Said::Paths(paths) => paths.iter().any(|p| news(&at, &root, p)),
                    });
                    if news {
                        seen.store(true, Ordering::Release);
                        wake();
                    }
                }
            })
        } else {
            name.spawn(move || {
                let mut last = look(&at);
                while !stopped.load(Ordering::Relaxed) {
                    std::thread::sleep(POLL_EVERY);
                    let now = look(&at);
                    if now != last {
                        last = now;
                        seen.store(true, Ordering::Release);
                        wake();
                    }
                }
            })
        };
        Watch { dir, changed, stop, events: watcher }
    }

    /// The folder watched.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Whether the system says (rather than the folder being looked at
    /// every half minute).
    #[cfg(test)]
    pub fn by_events(&self) -> bool {
        self.events.is_some()
    }

    /// Whether the folder changed since last asked.
    pub fn take(&self) -> bool {
        self.changed.swap(false, Ordering::AcqRel)
    }

    /// Whether it did, left to be asked again.
    pub fn pending(&self) -> bool {
        self.changed.load(Ordering::Acquire)
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Whether a change the system says `p` had is another app's: not a
/// hidden file's, nor one of the app's temporary files, nor a file still
/// as the app left it. `root` is `dir` as the system names it (macOS says
/// `/private/var` for `/var`).
fn news(dir: &Path, root: &Path, p: &Path) -> bool {
    let Some(rel) = p.strip_prefix(root).or_else(|_| p.strip_prefix(dir)).ok() else { return true };
    let hidden = rel.components().any(|c| matches!(c, Component::Normal(n) if n.to_string_lossy().starts_with('.')));
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let temporary = name.ends_with(".md.tmp") || name.ends_with(".tmp-carnet");
    !(hidden || temporary || store::is_own(&dir.join(rel)))
}

/// What the folder holds, as one number: each file's and folder's path,
/// and each file's modified time and length. Hidden ones are left out, as
/// the notes list leaves them, and so is what's past eight folders deep.
fn look(dir: &Path) -> u64 {
    fn walk(dir: &Path, depth: usize, sum: &mut u64) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            let Ok(meta) = fs::metadata(&path) else { continue };
            let mut h = DefaultHasher::new();
            path.hash(&mut h);
            if meta.is_dir() {
                if depth < 8 {
                    walk(&path, depth + 1, sum);
                }
            } else {
                (meta.len(), meta.modified().ok()).hash(&mut h);
            }
            // Added, not hashed in turn: the order a folder lists its
            // files in is the file system's.
            *sum = sum.wrapping_add(h.finish());
        }
    }
    let mut sum = 0;
    walk(dir, 0, &mut sum);
    sum
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("carnet-watch-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Work")).unwrap();
        dir
    }

    /// Whether the watch says the folder changed within `within`.
    fn says(watch: &Watch, within: Duration) -> bool {
        let t0 = std::time::Instant::now();
        while t0.elapsed() < within {
            if watch.take() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// What another app does is said, once it has settled: a note
    /// written, changed, renamed, deleted, a folder made. A hidden file,
    /// the app's temporary ones and a file the app wrote itself are not.
    #[test]
    fn the_system_says_what_another_app_does_and_not_what_the_app_did() {
        let dir = temp("events");
        let woke = Arc::new(AtomicUsize::new(0));
        let w = woke.clone();
        let watch = Watch::start(dir.clone(), move || {
            w.fetch_add(1, Ordering::Relaxed);
        });
        assert!(watch.by_events(), "the system watches the folder");
        assert!(watch.take(), "the start counts");
        // The system may still be telling of the folder's making.
        std::thread::sleep(Duration::from_millis(500));
        watch.take();
        let long = Duration::from_secs(5);
        let quiet = Duration::from_secs(1);
        let changes: [(&str, &dyn Fn()); 6] = [
            ("written", &|| fs::write(dir.join("New.md"), "# New\n").unwrap()),
            ("changed", &|| fs::write(dir.join("New.md"), "# New\n\nmore\n").unwrap()),
            ("renamed", &|| fs::rename(dir.join("New.md"), dir.join("Work/Newer.md")).unwrap()),
            ("deleted", &|| fs::remove_file(dir.join("Work/Newer.md")).unwrap()),
            ("a folder", &|| fs::create_dir(dir.join("Home")).unwrap()),
            ("a folder renamed", &|| fs::rename(dir.join("Home"), dir.join("House")).unwrap()),
        ];
        for (what, change) in changes {
            let before = woke.load(Ordering::Relaxed);
            change();
            assert!(says(&watch, long), "{what}");
            assert!(woke.load(Ordering::Relaxed) > before, "{what}: woken");
        }
        // Ten writes at once: one change said.
        for i in 0..10 {
            fs::write(dir.join(format!("Burst {i}.md")), "# Burst\n").unwrap();
        }
        assert!(says(&watch, long));
        assert!(!says(&watch, quiet), "a burst is said once");

        fs::write(dir.join(".pinned"), "x").unwrap();
        fs::write(dir.join("Work/Draft.md.tmp"), "x").unwrap();
        fs::remove_file(dir.join("Work/Draft.md.tmp")).unwrap();
        assert!(!says(&watch, quiet), "hidden and temporary files are no news");

        // The app's own: written, renamed, deleted.
        let own = dir.join("Work/Mine.md");
        fs::write(&own, "# Mine\n").unwrap();
        store::mark_own(&own);
        assert!(!says(&watch, quiet), "written by the app");
        let moved = dir.join("Mine.md");
        fs::rename(&own, &moved).unwrap();
        store::mark_own(&own);
        store::mark_own(&moved);
        assert!(!says(&watch, quiet), "renamed by the app");
        // Changed since by another app: news.
        std::thread::sleep(Duration::from_millis(20));
        fs::write(&moved, "# Mine\n\nedited elsewhere\n").unwrap();
        assert!(says(&watch, long), "the app's file, changed elsewhere");
        drop(watch);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Where the system won't watch, the folder is looked at whole, every
    /// so often; and the sum it takes changes with what another app does.
    #[test]
    fn without_the_systems_word_the_folder_is_looked_at() {
        let dir = temp("poll");
        let watch = Watch::start_as(dir.clone(), || {}, false);
        assert!(!watch.by_events());
        assert!(watch.take(), "the start counts");
        assert!(!says(&watch, POLL_EVERY * 3));
        fs::write(dir.join("New.md"), "# New\n").unwrap();
        assert!(says(&watch, Duration::from_secs(5)));
        let mut last = look(&dir);
        let mut seen = |what: &str, change: &dyn Fn()| {
            std::thread::sleep(Duration::from_millis(20));
            change();
            let now = look(&dir);
            assert_ne!(now, last, "{what}");
            last = now;
        };
        seen("changed", &|| fs::write(dir.join("New.md"), "# New\n\nmore\n").unwrap());
        seen("renamed", &|| fs::rename(dir.join("New.md"), dir.join("Newer.md")).unwrap());
        seen("deleted", &|| fs::remove_file(dir.join("Newer.md")).unwrap());
        seen("a folder", &|| fs::create_dir(dir.join("Home")).unwrap());
        fs::write(dir.join(".pinned"), "x").unwrap();
        assert_eq!(look(&dir), last, "a hidden file is not a change");
        drop(watch);
        let _ = fs::remove_dir_all(&dir);
    }
}
