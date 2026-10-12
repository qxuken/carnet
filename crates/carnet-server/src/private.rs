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
    dir(data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(data)?.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            std::fs::set_permissions(data, std::fs::Permissions::from_mode(0o700))?;
            eprintln!("carnet-server: {} was open to other users of this machine (mode {mode:o}): now to this one alone (700)", data.display());
        }
    }
    Ok(())
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
