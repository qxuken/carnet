//! User-requested materialization. Only this worker may wait on a cloud
//! provider; no UI thread joins it, including when the app closes.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::{AtomicBool, AtomicUsize, Ordering}, mpsc};
use kui_native::Waker;

#[derive(Default)]
pub struct Download {
    job: Option<Job>,
    pub error: Option<String>,
}

struct Job {
    root: PathBuf,
    result: mpsc::Receiver<Result<(), String>>,
    stop: Arc<AtomicBool>,
    count: Arc<AtomicUsize>,
}

impl Download {
    pub fn busy(&self) -> bool { self.job.is_some() }

    pub fn status(&self) -> String {
        match &self.job {
            Some(j) if j.stop.load(Ordering::Relaxed) => "Stopping after the current file…".into(),
            Some(j) => format!("Downloading… {} files ready", j.count.load(Ordering::Relaxed)),
            None => self.error.clone().unwrap_or_default(),
        }
    }

    pub fn cancel(&self) {
        if let Some(j) = &self.job { j.stop.store(true, Ordering::Relaxed); }
    }

    pub fn start(&mut self, root: PathBuf, path: PathBuf, waker: Option<Waker>) {
        if self.busy() { return; }
        self.error = None;
        let (tx, result) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let count = Arc::new(AtomicUsize::new(0));
        let (stopped, ready) = (stop.clone(), count.clone());
        let spawned = std::thread::Builder::new().name("carnet-download".into()).spawn(move || {
            let result = materialize(&path, &stopped, &ready)
                .map_err(|e| format!("Couldn't download {}: {e}", path.display()));
            let _ = tx.send(result);
            if let Some(w) = waker { w.wake(); }
        });
        match spawned {
            Ok(_) => self.job = Some(Job { root, result, stop, count }),
            Err(e) => self.error = Some(format!("Couldn't start download: {e}")),
        }
    }

    /// A result names its vault, so switching folders during a download
    /// cannot open a note or apply a result to the wrong vault.
    pub fn poll(&mut self) -> Option<PathBuf> {
        let job = self.job.as_ref()?;
        let result = match job.result.try_recv() {
            Ok(r) => r,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => Err("The download stopped unexpectedly. Retry when ready.".into()),
        };
        self.error = result.err();
        self.job.take().map(|j| j.root)
    }
}

impl Drop for Download {
    fn drop(&mut self) { self.cancel(); }
}

fn materialize(path: &Path, stop: &AtomicBool, count: &AtomicUsize) -> io::Result<()> {
    // Thread-scoped: even if inherited from a caller that opted out,
    // this explicit user request may materialize files and directories.
    let _policy = crate::disk::allow_downloads()?;
    fn visit(path: &Path, stop: &AtomicBool, count: &AtomicUsize) -> io::Result<()> {
        if stop.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "Download cancelled"));
        }
        crate::disk::request_download(path)?;
        let started = std::time::Instant::now();
        let meta = loop {
            match fs::symlink_metadata(path) {
                Ok(meta) => break meta,
                Err(e) if e.kind() == io::ErrorKind::NotFound && crate::disk::has_cloud_placeholder(path)
                    && started.elapsed() < std::time::Duration::from_secs(120) => {
                    if stop.load(Ordering::Relaxed) { return Err(io::Error::new(io::ErrorKind::Interrupted, "Download cancelled")); }
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
                Err(e) => return Err(e),
            }
        };
        // Never follow links out of the selected folder, or into cycles.
        if meta.is_symlink() { return Ok(()); }
        if meta.is_dir() {
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                if entry.file_name() == ".git" { continue; }
                let path = crate::disk::cloud_original(&entry.path()).unwrap_or_else(|| entry.path());
                visit(&path, stop, count)?;
            }
        } else if meta.is_file() {
            let mut file = fs::File::open(path)?;
            let mut buffer = [0; 64 * 1024];
            loop {
                if stop.load(Ordering::Relaxed) {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "Download cancelled"));
                }
                if file.read(&mut buffer)? == 0 { break; }
            }
            count.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }
    // The explicitly selected item may itself be a symlink (including a
    // vault chosen through one). Resolve that one; nested links stay out.
    let selected = if fs::symlink_metadata(path).is_ok_and(|m| m.is_symlink()) {
        fs::canonicalize(path)?
    } else { path.to_path_buf() };
    visit(&selected, stop, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_reads_without_rewriting_and_cancel_preserves_files() {
        let root = std::env::temp_dir().join(format!("carnet-download-{}", std::process::id()));
        fs::create_dir_all(root.join("folder")).unwrap();
        let path = root.join("folder/note.md");
        fs::write(&path, "# Original\n").unwrap();
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&root, root.join("cycle")).unwrap();
        let stop = AtomicBool::new(false);
        let count = AtomicUsize::new(0);
        materialize(&root, &stop, &count).unwrap();
        assert_eq!(count.load(Ordering::Relaxed), 1);
        #[cfg(unix)]
        {
            let link = root.join("selected.md");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            materialize(&link, &stop, &count).unwrap();
            assert_eq!(count.load(Ordering::Relaxed), 2);
        }
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
        stop.store(true, Ordering::Relaxed);
        assert_eq!(materialize(&root, &stop, &count).unwrap_err().kind(), io::ErrorKind::Interrupted);
        assert_eq!(fs::read_to_string(&path).unwrap(), "# Original\n");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn download_failure_can_be_retried_and_keeps_its_vault_identity() {
        let root = std::env::temp_dir().join(format!("carnet-download-retry-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("later.md");
        let mut download = Download::default();
        let wait = |download: &mut Download| {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                if let Some(root) = download.poll() { return root; }
                assert!(std::time::Instant::now() < until);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        download.start(root.clone(), path.clone(), None);
        assert_eq!(wait(&mut download), root);
        assert!(download.error.is_some());
        fs::write(&path, "Ready").unwrap();
        download.start(root.clone(), path, None);
        assert_eq!(wait(&mut download), root);
        assert!(download.error.is_none());
        fs::remove_dir_all(root).unwrap();
    }
}
