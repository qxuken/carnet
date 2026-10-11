#![cfg_attr(windows, windows_subsystem = "windows")]

mod app;
// The page's editing lives in carnet-core, shared with the web editor.
use carnet_core::{history, page};
mod clip;
mod design;
mod icons;
mod images;
#[cfg(target_os = "macos")]
mod imageio;
mod keys;
mod look;
mod oscursor;
mod settings;
mod signup;
mod slash;
mod store;
mod pictures;
mod sync;
#[cfg(test)]
mod sync_tests;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // `carnet --tokens docs/tokens.json`: the design system's tokens, as
    // JSON, for the design reference. (A file, not stdout: a windowed app
    // on Windows has no console to print to.)
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--tokens") {
        let path = args.get(i + 1).map_or("tokens.json", String::as_str);
        std::fs::write(path, design::tokens_json())?;
        return Ok(());
    }
    // `carnet --icons <dir>`: every icon as an SVG, for the reference.
    if let Some(i) = args.iter().position(|a| a == "--icons") {
        let dir = std::path::PathBuf::from(args.get(i + 1).map_or("icons", String::as_str));
        std::fs::create_dir_all(&dir)?;
        for g in icons::Glyph::ALL {
            std::fs::write(dir.join(format!("{}.svg", g.slug())), g.svg("#2f2a27"))?;
        }
        return Ok(());
    }
    let settings = settings::Settings::load();
    // Asked of the system once, as the window opens: the blurred desktop
    // behind it where there is one, and the library drawn over that.
    let backdrop = if settings.translucent { kui_native::Backdrop::Blur } else { kui_native::Backdrop::Opaque };
    // The chosen folder may be on a drive that is not there today: then
    // the default folder opens, and says so, rather than nothing at all.
    let chosen = store::default_dir(settings.dir.as_deref());
    let (store, missed) = match store::Store::open(chosen.clone()) {
        Ok(s) => (s, None),
        Err(e) => (store::Store::open(store::documents_dir())?, Some((chosen, e))),
    };
    // Sync, as it was left: on if this device was set up (sync.txt beside
    // the settings, its keys in the system's keychain), off otherwise.
    let sync_dir = settings.dir();
    let mut app = app::Carnet::with_settings(store, settings);
    if let Some(dir) = sync_dir {
        app.sync = sync::Sync::load(dir.join("sync.txt"), std::sync::Arc::new(sync::Keychain::new(dir)));
    }
    if let Some((dir, e)) = missed {
        app.error = Some(format!("Couldn't open {} ({e}); showing Documents/Carnet", dir.display()));
    }
    kui_native::app("Carnet")
        // The app draws its own titlebar: the page runs to the top edge.
        // A toolbar's height, so on a Mac the traffic lights sit level with
        // the strip's tools and the library's name, not above them.
        .chrome(kui_native::Chrome::Custom)
        .titlebar(kui_native::Titlebar::Tall)
        .size(1100.0, 740.0)
        .min_size(360.0, 420.0)
        .backdrop(backdrop)
        // The title bar's and the taskbar's on Windows, the window
        // manager's on X11; a Mac takes the bundle's `.icns` instead.
        // `assets/icon/render.sh` makes both from the SVG.
        .icon_bytes(include_bytes!("../assets/icon/carnet-256.png"))
        .run(app)
}