//! What the server keeps on disk is its own user's alone (docs/SYNC.md
//! §6): the data folder `0700`, the folders in it `0700`, the databases
//! and pictures `0600`. Sealed or public as all of it is, who has an
//! account, how big, and when it last wrote are still nobody else's on
//! the machine. (Elsewhere than unix, the folder's own permissions.)

use std::io;
use std::path::Path;

/// The data folder made, its own user's alone; one from before that was
/// open to others is closed, and said once (it isn't open the next time).
pub fn data_dir(data: &Path) -> io::Result<()> {
    dir(data).map_err(|e| not_ours(data, e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(data)?.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            std::fs::set_permissions(data, std::fs::Permissions::from_mode(0o700)).map_err(|e| not_ours(data, e))?;
            eprintln!("carnet-server: {} was open to other users of this machine (mode {mode:o}): now to this one alone (700)", data.display());
        }
    }
    // Written in, as the databases will be: a folder another user owns,
    // closed already, fails here, in words that say what to do.
    let probe = data.join(".carnet-write-check");
    create().open(&probe).map_err(|e| not_ours(data, e))?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// What the system's "not permitted" means for the data folder: another
/// user owns it (a bind mount, as Unraid's appdata or a folder made by
/// root), and the server runs as its own (the image's is uid 10001).
fn not_ours(data: &Path, e: io::Error) -> io::Error {
    if e.kind() != io::ErrorKind::PermissionDenied {
        return e;
    }
    #[cfg(unix)]
    let owner = {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(data).map(|m| format!(" (it belongs to uid {}, group {})", m.uid(), m.gid())).unwrap_or_default()
    };
    #[cfg(not(unix))]
    let owner = String::new();
    io::Error::new(
        e.kind(),
        format!(
            "{} isn't this server's to keep its data in{owner}: {e}. Give it to the user the server runs as \
             (the container's is uid 10001: chown -R 10001:10001 on the folder you mounted), \
             or run the server as the folder's owner (docker run --user UID:GID)",
            data.display()
        ),
    )
}

/// A folder made (and those it's in), its own user's alone; one there
/// already is left as it is.
pub fn dir(path: &Path) -> io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(path)
}

/// A file there made its own user's alone (`0600`), if it isn't. A
/// database's before SQLite makes its WAL, which takes the database's.
pub fn file(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::metadata(path)?.permissions().mode() & 0o777 != 0o600 {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// How to make a new file its own user's alone from the start.
pub fn create() -> std::fs::OpenOptions {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    /// A data folder the server can't write in, as one another user owns:
    /// the start fails saying whose it is and what to do, not the bare
    /// "Operation not permitted" it once was.
    #[test]
    fn a_data_folder_not_its_own_is_said_plainly() {
        let dir = std::env::temp_dir().join(format!("carnet-not-ours-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        // Root writes anywhere: nothing to show there.
        if std::fs::File::create(dir.join("x")).is_ok() {
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        let e = super::data_dir(&dir).unwrap_err().to_string();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(e.contains("isn't this server's") && e.contains("uid 10001") && e.contains("--user"), "{e}");
        assert!(e.contains("it belongs to uid"), "{e}");
    }
}
