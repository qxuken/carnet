//! The shipped fonts checked to be fonts: they're compiled in (`look.rs`),
//! and were once kept in Git LFS, whose pointer files a checkout of those
//! commits without LFS has in their place: an app built from them would
//! silently fall back to the system's faces.
//!
//! Windows only: the executable's icon and version resource
//! (`packaging/windows/carnet.rc`). Cross builds from a Mac or Linux take
//! LLVM's `llvm-rc` from the PATH (`scripts/package.nu` puts Homebrew's
//! there); without one the build goes on with no icon in the .exe, and the
//! window still has its own from `assets/icon/carnet-256.png`.

fn main() {
    fonts();
    println!("cargo:rerun-if-changed=packaging/windows/carnet.rc");
    println!("cargo:rerun-if-changed=assets/icon/carnet.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let num = |k: &str| std::env::var(k).unwrap_or_else(|_| "0".into());
    let defines = [
        format!("CARNET_VERSION_MAJOR={}", num("CARGO_PKG_VERSION_MAJOR")),
        format!("CARNET_VERSION_MINOR={}", num("CARGO_PKG_VERSION_MINOR")),
        format!("CARNET_VERSION_PATCH={}", num("CARGO_PKG_VERSION_PATCH")),
        format!("CARNET_VERSION=\"{}\"", num("CARGO_PKG_VERSION")),
    ];
    embed_resource::compile("packaging/windows/carnet.rc", &defines).manifest_optional().unwrap();
}

fn fonts() {
    println!("cargo:rerun-if-changed=assets/fonts");
    let Ok(dir) = std::fs::read_dir("assets/fonts") else { return };
    for entry in dir.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "ttf") {
            let head = std::fs::read(&path).unwrap_or_default();
            if head.starts_with(b"version https://git-lfs") {
                panic!("{} is a Git LFS pointer, not the font: run `git lfs pull` (or check out with LFS)", path.display());
            }
        }
    }
}
