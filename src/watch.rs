//! The notes folder looked at from a thread of its own, every couple of
//! seconds, for what other apps do to it: a note written, changed,
//! renamed or deleted, a folder made, a picture put beside. The app is
//! woken to catch up (a rescan) only when something did change: a file
//! another app writes shows without the window having to come to the
//! front, and a folder of 10,000 notes left as it is costs the window
//! nothing.

use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// How often the folder is looked at.
pub const EVERY: Duration = if cfg!(test) { Duration::from_millis(50) } else { Duration::from_secs(2) };

pub struct Watch {
    dir: PathBuf,
    changed: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

impl Watch {
    /// `dir` watched, `wake` called from the watching thread when
    /// something in it changed. The first look counts as a change: what
    /// came between the folder's last reading and the watch starting.
    pub fn start(dir: PathBuf, every: Duration, wake: impl Fn() + Send + 'static) -> Self {
        let changed = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen, stopped, at) = (changed.clone(), stop.clone(), dir.clone());
        let _ = std::thread::Builder::new().name("carnet-watch".into()).spawn(move || {
            let mut last = None;
            while !stopped.load(Ordering::Relaxed) {
                let now = look(&at);
                if last != Some(now) {
                    last = Some(now);
                    seen.store(true, Ordering::Release);
                    wake();
                }
                std::thread::sleep(every);
            }
        });
        Watch { dir, changed, stop }
    }

    /// The folder watched.
    pub fn dir(&self) -> &Path {
        &self.dir
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

    /// A file written, changed, renamed or deleted, or a folder made,
    /// changes what the folder is seen to hold; a hidden one doesn't.
    /// Looking at 10,000 notes is quick (and off the window's thread).
    #[test]
    fn what_another_app_does_is_seen() {
        let dir = std::env::temp_dir().join(format!("carnet-watch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Bulk")).unwrap();
        for i in 0..10_000 {
            fs::write(dir.join("Bulk").join(format!("Note {i}.md")), format!("# Note {i}\n")).unwrap();
        }
        let t0 = std::time::Instant::now();
        let mut last = look(&dir);
        let took = t0.elapsed();
        println!("10,000 notes looked at in {took:?}");
        assert!(took < Duration::from_secs(3), "{took:?}");
        let mut seen = |what: &str, change: &dyn Fn()| {
            // A later modified time, wherever the clock is coarse.
            std::thread::sleep(Duration::from_millis(20));
            change();
            let now = look(&dir);
            assert_ne!(now, last, "{what}");
            last = now;
        };
        seen("written", &|| fs::write(dir.join("New.md"), "# New\n").unwrap());
        seen("changed", &|| fs::write(dir.join("New.md"), "# New\n\nmore\n").unwrap());
        seen("renamed", &|| fs::rename(dir.join("New.md"), dir.join("Newer.md")).unwrap());
        seen("deleted", &|| fs::remove_file(dir.join("Newer.md")).unwrap());
        seen("a folder", &|| fs::create_dir(dir.join("Work")).unwrap());
        fs::write(dir.join(".pinned"), "x").unwrap();
        assert_eq!(look(&dir), last, "a hidden file is not a change");

        // The thread: a change wakes the app, and is taken once.
        let woke = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let w = woke.clone();
        let watch = Watch::start(dir.clone(), EVERY, move || {
            w.fetch_add(1, Ordering::Relaxed);
        });
        let wait = |watch: &Watch| {
            (0..400).any(|_| {
                let t = watch.take();
                if !t {
                    std::thread::sleep(EVERY / 5);
                }
                t
            })
        };
        assert!(wait(&watch), "the first look counts");
        let before = woke.load(Ordering::Relaxed);
        fs::write(dir.join("Later.md"), "# Later\n").unwrap();
        assert!(wait(&watch), "a file written is seen");
        assert!(woke.load(Ordering::Relaxed) > before);
        assert!(!watch.take());
        drop(watch);
        let _ = fs::remove_dir_all(&dir);
    }
}
