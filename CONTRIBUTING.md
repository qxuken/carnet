# Contributing to Carnet

Thanks for taking an interest. This page covers how to build Carnet, run its tests, find your way around the code, and send a change. For what Carnet is and how to use it, see the [README](README.md); for how sync works, [docs/SYNC.md](docs/SYNC.md).

## Where development happens

Carnet lives on GitHub, at [github.com/qxuken/carnet](https://github.com/qxuken/carnet): the code, the issues, the checks, the releases and the server's images.

Issues and pull requests are welcome. A pull request is checked by GitHub Actions (`ci.yml`, below) and, once it passes and has been read, merged into `main` there.

Security problems should not go in a public issue: see [SECURITY.md](SECURITY.md).

## Building

You need a recent stable [Rust](https://rustup.rs) (the 2024 edition, so 1.85 or later). On Linux, also `pkg-config` and ALSA's headers (`libasound2-dev` on Debian and Ubuntu), which the UI framework's sound links against.

```bash
cargo run --release
```

`cargo build` and `cargo test` cover the whole workspace: the app, `carnet-core`, `carnet-sync`, `carnet-server` and `carnet-web`. `fuzz/` is a workspace of its own.

Carnet's UI framework is [kui](https://github.com/qxuken/kui), used from crates.io as `kui-native` at the exact version `Cargo.toml` pins, so a release of Carnet names the kui it was built with. To try Carnet against a local checkout of kui, point the dependency at it with `path = "../kui/crates/kui-native"`.

The fonts in `assets/fonts/` (Source Serif 4 and Inter, under the SIL Open Font License) are ordinary files in the repository, built into the binary.

Useful while developing:

- `CARNET_DIR=/tmp/notes` points the app at another notes folder, and `CARNET_CONFIG=/tmp/carnet.txt` at another settings file, so a development build never touches your own notes. Both together give a second, independent Carnet, with sync keys of its own.
- `CARNET_THEME=light` or `CARNET_THEME=dark` pins the theme.

### The web app

The web app in `web/` is Svelte 5 and TypeScript, with the notes handled by `crates/carnet-web` compiled to WebAssembly. It needs Node (CI uses Node 24), Rust's `wasm32-unknown-unknown` target, and `wasm-bindgen-cli` at the version `carnet-web` pins:

```bash
rustup target add wasm32-unknown-unknown
cargo install --locked wasm-bindgen-cli --version 0.2.129
```

Then, in `web/`:

| Command | What it does |
|---|---|
| `npm ci` | installs the dependencies |
| `npm run wasm` | builds `crates/carnet-web` as wasm into `src/wasm/`, and copies the fonts and icon into `public/` |
| `npm run dev` | Vite's development server; it passes `/v1` (the API and the sync socket) on to a `carnet-server` at `CARNET_SERVER`, or `http://127.0.0.1:8787` |
| `npm run check` | type-checks the app (`svelte-check`) |
| `npm run build` | the wasm, then the production build in `dist/`, with Brotli and gzip copies beside each file (needs the `brotli` command) |

For `npm run dev`, run a server beside it whose origin is the page's, since a device signs its login for the address it sees:

```bash
cargo run -p carnet-server -- --listen 127.0.0.1:8787 --origin http://localhost:5173
```

To serve a production build the way a self-hoster would:

```bash
cargo run --release -p carnet-server -- --web web/dist
```

`web/fonts/` holds the desktop's fonts as WOFF2; `web/scripts/fonts.sh` remakes them when the desktop's fonts change.

## Testing

```bash
cargo test
```

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Both must pass before a change is merged; CI runs them on every push to `main` and every pull request (with `--locked`). CI also builds `carnet-core` and `carnet-sync` for the browser, which keeps the UI framework, files and threads out of what the web app is built on:

```bash
cargo build -p carnet-core -p carnet-sync --target wasm32-unknown-unknown
```

If you touch `web/`, run `npm run check` there too.

The app's UI tests are headless (`src/app_tests.rs`, through `kui_native::testing::Drive`): they open no window. `src/sync_tests.rs` syncs two notes folders through the server's hub in memory.

`cargo test` also runs property tests: random notes, edits, orders of arrival and folders of notes, shrunk by [proptest](https://proptest-rs.github.io/proptest/) to the smallest case that fails, which is then kept in `proptest-regressions/` and tried first from then on. They hold that nothing panics on any text, that the Markdown round trip settles, that copies of a note converge whatever order their changes arrive in, and that no word typed here or in another app is lost through saves, merges, renames, restarts and the trash. A deeper run asks for more cases:

```bash
env PROPTEST_CASES=10000 cargo test --release
```

Frame cost on a note with 2,000 lines:

```bash
cargo test --release big_note -- --nocapture
```

Replay a real table-heavy note headlessly (the file is only read):

```bash
env CARNET_PERF_NOTE=/path/to/note.md cargo test -p carnet --release supplied_note_frame_cost -- --ignored --nocapture
```

`CARNET_PERF_TABLE_WIDTH=page|window|scroll` picks the table mode;
`CARNET_PERF_FRAMES` changes the 60-frame samples. Steady frames, wheel
scrolling through the note, and typing in its largest table must each
average under 8 ms, with their 95th percentiles under 8 ms too.
`CARNET_PERF_CAPTURE=/tmp/table.png` captures the first table afterwards.
To inspect the phases instead, set
`CARNET_PERF_TRACE=/tmp/note.kuitrace`; recording adds overhead, so that
run reports timing without enforcing the budget.

Measure the actual native window and renderer over a disposable copy of the
same note (the original file and user settings are untouched):

```bash
cargo build --release -p carnet
target/release/carnet --native-perf /path/to/note.md /tmp/native-perf window 10
```

The last arguments select `page|window|scroll` and seconds per workload.
After warming up, it scrolls through the note, then continuously commits and
backspaces text in its largest table, checking that every operation changed
the cell. It writes `metadata.json` and `frames.csv`. Inputs go through core
dispatch and the real app handlers; they bypass OS event delivery. The driver
uses `Launcher::run`, including the production display-link pacer, and injects
input in `App::before_frame`, while the previous event tree is still intact.
`PumpRunner` disables that pacer and must not stand in for this measurement.
Focus counts are saved for each phase. Keep the window visible on
the display being measured and avoid other benchmark/build work during a run.

To check that scrolling actually stops rendering, set
`CARNET_PERF_IDLE_CHECK=1` with the same command. It enables the latency HUD,
scrolls for the requested duration, then leaves the window idle for five
seconds without requesting animation frames. `metadata.json` records
`idle_steady_frames` over the middle 3.5 seconds; more than 12 fails the run.
A visible caret may blink; an offscreen caret should produce no frames.
Check the per-phase focus counts before trusting a native idle result.

These CSV timings measure CPU input, view, layout, encoding/submission and
surface acquisition, and post-submit text-cache maintenance (`maintenance_ms`).
Maintenance can overlap GPU execution, so adding CPU and GPU work is a
conservative work sum, not wall-clock duration. **They do not measure GPU completion or presentation.**
Acquisition can wait for vsync; it is not additional CPU work. The `elapsed_ms` column is read at the next frame's entry;
`frame_started_ms` records input sampling for the labelled frame. Neither
column alone establishes physical display cadence. For actual GPU execution and displayed frame
intervals on macOS, record a Metal System Trace:

```bash
xcrun xctrace record --template 'Metal System Trace' --output /tmp/native.trace --time-limit 30s --launch -- "$PWD/target/release/carnet" --native-perf /path/to/note.md /tmp/native-perf window 10
xcrun xctrace export --input /tmp/native.trace --toc --output /tmp/native-perf/metal-toc.xml
xcrun xctrace export --input /tmp/native.trace --xpath '/trace-toc/run[@number="1"]/data/table[@schema="metal-gpu-intervals" or @schema="displayed-surfaces-interval" or @schema="display-vsyncs-interval"]' --output /tmp/native-perf/metal-data.xml
```

`scripts/analyze-native-perf.py ROOT` compares `ROOT/validated-{page,window,scroll}`
CSV runs with `ROOT/gpu-{page,window,scroll}` Metal exports and writes
`ROOT/summary.json`. It filters the target process, reports physical surface
coverage before trusting cadence, counts gaps between physical presentations
(including gaps with no owned surface), unions overlapping GPU intervals, and keeps
GPU work, displayed intervals and latency separate. GPU passes are grouped by
native window/frame label, including labelled passes without a hardware GPU
frame counter. Native GPU passes carry
window/frame labels. The analyzer follows those texture writes through
WindowServer's texture reads and scanout-buffer writes, retaining the version
the compositor sampled even when the client's drawable is later reused.
Rotating compositor buffers containing the same app frame do not count as
new app updates. WindowServer versions are resolved per command buffer. A
retained scanout buffer can be rewritten in place: actual hardware VSYNC
pulses sample its resource history, and only a new resolved app frame counts
as an update. Direct client-label changes exclude those in-place updates. Narrative `Frame` numbers and GPU frame counters are separate
namespaces and must not be joined by number. Its `frame_start_to_display_ms` includes app input,
build, layout, GPU and compositor latency; it bypasses OS input delivery.
`frame_start_to_gpu_completion_ms` includes drawable acquisition and GPU queue
waits. The CPU/GPU work sum excludes those waits and is not an end-to-end frame
latency or proof that each refresh was met.
Metal's built-in CPU-to-display counter starts later and is reported
separately. Metal recording itself adds overhead; preserve an uninstrumented
native run too.

For a single native capture and its summary, run the Nushell helper:

```bash
nu scripts/record-native-perf.nu /path/to/note.md /tmp/native-window window 10
python3 scripts/analyze-native-perf.py /tmp/native-window --single-case --max-latency-ms 20 --max-missed-refreshes 0
```

The second command fails if physical display coverage is insufficient,
source frames regress, or either sustained workload exceeds either limit.
Each missed presentation includes nearby CPU/acquire/GPU timings and hardware
VSYNC pulses inside the gap, when available. Independent direct client-label
cadence is retained as a diagnostic when compositor resource attribution is
ambiguous; it cannot establish app frame-start latency.
Add `--max-frame-latency-ms`
to enforce the correlated frame-start latency too; this also requires at
least 95% frame correlation. Metal's built-in latency gate also requires at
least 95% counter coverage. `CARNET_GPU_DISPLAY_ONLY=1` records GPU and Display
without API tracing; verify that frame labels and scanout coverage are retained.
`CARNET_PRESENTATION_ONLY=1` records only Display (no GPU timing or frame
correlation); sparse client labels make its coverage insufficient on some
composited windows. `CARNET_METAL_TEMPLATE` selects another trace template.
Use identical instruments for before/after comparisons. `KUI_FRAME_LATENCY=1|2` and
`KUI_FRAME_PACING=0|HZ` are forwarded explicitly by the capture helper for A/B
comparisons. On macOS, native-runloop windows acquire Metal drawables on a
worker by default, leaving AppKit free to deliver input; `KUI_ACQUIRE_THREAD=0`
selects ordinary synchronous acquisition for a control. The display link
requests the current monitor’s reported cadence, updating when it changes;
this is a preference, not proof of scanout rate. `KUI_WINDOW_AT=X,Y` pins the window to the intended display;
`KUI_SPECIALIZE=0` disables Kui's Metal quad specialization for a renderer
control. Capture folders and traces must be new; earlier evidence is kept.
`CARNET_PERF_WINDOW=solid|translucent` selects the benchmark window backdrop
(default translucent), using the app's existing setting in its disposable store.
The helper saves display configuration and power state with each capture.
The stock Metal System Trace template can produce many gigabytes of temporary
kernel data; budget disk space for recording and saving, not just the final trace.

**Where a frame's time goes.** kui records a running app as a timeline: each frame's phases and why it ran, and Carnet's own spans (`library`, `toolbar`, `page`, `settings`, `sync`, `folder watch`, `autosave`, the pictures let go and handed over), a picture's decode timed on its reader thread (`decode picture`), what the pictures held come to, the moments a note opened or was saved or the folder was read again, which thread woke the window (`wake: sync`, `wake: pictures`, `wake: folder watch`, …) and what asked for a frame (`request: caret reveal`, …). Nothing is timed while nobody records. In a debug build, F12 opens the devtools, whose Perf tab shows it live and records on its button; or write a session to a file and open it in Perfetto or Chrome's tracing:

```bash
env KUI_PERF_TRACE=carnet.pftrace cargo run --release
```

**The server with Postgres and S3.** `carnet-server`'s storage and migration tests also run against a real Postgres and an S3-compatible bucket when they're given one; without them those tests skip themselves. [The server's README](crates/carnet-server/README.md#tests) has the commands.

**The sync protocol's fixtures.** `crates/carnet-sync/protocol/` holds how each protocol version's frames encode, byte for byte, so that apps and servers of different versions keep understanding each other ([docs/SYNC.md](docs/SYNC.md) §14). A change to the wire fails the test with what to do: raise the protocol number, keep the old fixture, and bless the new one with `CARNET_BLESS=1 cargo test -p carnet-sync wire`.

### Fuzzing

`fuzz/` holds [cargo-fuzz](https://rust-fuzz.github.io/book/cargo-fuzz.html) targets (libFuzzer, nightly Rust) for the slow, wide search the property tests are too quick for. They feed any bytes to the Markdown reader (`markdown`), the inline edits (`inline`), the history's comparison (`compare`), a note's CRDT (`note_doc`), the replica's files (`replica`), the wire's frames and records (`wire`), and the server's hub (`hub`). Nothing may panic.

```bash
cargo install cargo-fuzz
```

```bash
cd fuzz && cargo +nightly fuzz run compare corpus/compare seeds/compare -- -max_total_time=300
```

A nightly CI job runs every target from the corpus earlier nights built up. A crashing input is kept in `fuzz/seeds/<target>/` along with its fix, so it is tried first from then on.

## Finding your way around

| Path | What |
|---|---|
| `src/main.rs` | the entry point, and `--tokens` / `--icons` / `--screenshots` exports |
| `src/shots.rs` | `--screenshots`: the README's pictures, taken headless over a demo library |
| `src/app.rs` | the app: state, notes, events |
| `src/app_page.rs` | the page's drawing, keys, pointer, paste and drop |
| `src/app_chrome.rs` | the library, the search and the toolbar |
| `src/app_settings.rs` | the settings page |
| `src/app_menu.rs` | the Mac's menu bar |
| `src/app_tests.rs` | headless UI tests |
| `src/store.rs` | the notes folder: atomic writes, renames, pins, the trash, attachments, edits made in other apps |
| `src/sync.rs` | sync on this device: off until set up in Settings → Sync; the session each frame, the keys in the system's keychain |
| `src/design.rs` | the design system: palettes, derived colours, type, space, radii, sizes, density, font pairings |
| `src/look.rs` | the settings turned into the `Look` every view paints with |
| `src/settings.rs` | the settings and their file |
| `src/images.rs` | pictures decoded off the UI thread and handed to kui |
| `src/files.rs` | files between devices (pictures, and documents notes link to), each a sealed blob on the server |
| `src/clip.rs` | the clipboard beyond text: pictures and copied files |
| `crates/carnet-core/` | the note's Markdown, with no UI in it, so it also builds for wasm |
| `crates/carnet-core/src/doc.rs` | the block model and its GFM Markdown form |
| `crates/carnet-core/src/inline.rs` | inline styles: which bytes are bold, italic, code, links, colour |
| `crates/carnet-core/src/page.rs` | the page editor's model: caret, selection across blocks, every edit, as plain functions (the desktop's and the web's) |
| `crates/carnet-core/src/history.rs` | undo and redo |
| `crates/carnet-core/src/compare.rs` | what changed between two versions of a note, for History |
| `crates/carnet-core/src/name.rs` | a note's file name from its title, the same on every device |
| `crates/carnet-core/src/export.rs` | Markdown to HTML, for copying |
| `crates/carnet-sync/` | each note as a [Loro](https://loro.dev) CRDT, the replica beside the notes folder, encryption, pairing, recovery, and the wire ([docs/SYNC.md](docs/SYNC.md)) |
| `crates/carnet-server/` | the sync server: the hub (logins, rooms, the change feed), SQLite or Postgres storage, files on disk or in S3, the binary; `tests/sim.rs` simulates devices, the server and a faulty network from a seed |
| `crates/carnet-web/` | the web app's Rust, as wasm ([docs/SYNC.md](docs/SYNC.md) §8) |
| `web/` | the web app (Svelte 5) |
| `fuzz/` | fuzz targets |

### The design system

Every colour, size, space and radius is a token in `src/design.rs`:

- **Palettes**: Linen (the default), Sage, Harbor, Blossom and Graphite, each light and dark. Each is five seed colours per mode; every other colour is derived, and the text colours are solved for contrast (muted 5:1, faint 3.3:1, accent text 4.5:1), which a test checks for all ten themes.
- **Type**: an interface scale (fixed) and a page scale that follows the reader's text size.
- **Space, radii and sizes**: one spacing ladder, three corner radii, fixed control and icon sizes.
- **Density**: Compact, Comfortable or Airy, stretching the page's rhythm and the library's rows.
- **Font pairings**: Book, Modern, Classic or System.

To export the tokens and the icons:

```bash
cargo run --release -- --tokens docs/tokens.json
```

```bash
cargo run --release -- --icons docs/icons
```

### The screenshots

The README's pictures of the desktop app (`docs/light.png`, `dark.png`, `settings.png` and `narrow.png`) are taken by the app itself, with no window: a demo library made in a temporary folder, the settings in memory, sync and the update check off, and the system pinned to a Mac's (light or dark, its accent, the faces Carnet ships, the traffic lights and rounded corners drawn on), so they come out the same on any machine. Take them again after a change that shows, and look at them before committing:

```bash
cargo run -p carnet -- --screenshots docs
```

It takes a few seconds and needs a GPU (a software one does). `docs/web.png`, the web app, is taken by hand. The demo library, the shots and the waiting are in `src/shots.rs`.

The app icon is a page of a small notebook: ruled lines, a clay margin rule, and a C in Source Serif sitting on a line (`assets/icon/carnet.svg`, with a dark twin beside it, and `carnet-small.svg` with fewer, heavier lines for 64 px and under). `assets/icon/render.sh` turns them into the PNGs, the Mac's `.icns` and Windows' `.ico`.

## Sending a change

- Keep a change to one thing, with its tests. A bug fix comes with a test that failed before it.
- Run `cargo test` and `cargo clippy --workspace --all-targets -- -D warnings` (and `npm run check` in `web/` if you touched it) before you push.
- Notes are people's own files: anything that reads or writes them must never lose a word or mangle Markdown written by another app. If your change touches the store, the Markdown or sync, say how you checked that.
- In the interface, use plain words rather than internal names, and let a button say what it does.

### Commit messages

The subject line says, in plain words, what is different afterwards, from the point of view of whoever uses it: "A heading's section collapses on the page, never in the file", "The first screen offers email and password only where the server takes them". No `feat:`-style prefixes. The body, when there is one, says why and how, in prose. Merges of a branch read "Merge <what it brings>".

## Continuous integration

Every workflow is GitHub Actions', in `.github/workflows/`, on GitHub's Ubuntu runners but for the `.dmg`, which is built on macOS.

| Workflow | When | What |
|---|---|---|
| `ci.yml` | every push to main and every pull request | the tests and clippy; `carnet-core` and `carnet-sync` for wasm; `carnet-server` with Postgres and S3 built in, against a real Postgres and bucket (service containers); the web app's wasm, type check, tests and build |
| `edge.yml` | every push to main | the server's image as main stands, for amd64, pushed to `ghcr.io/qxuken/carnet-server:main` (and `:<commit>`) |
| `fuzz.yml` | nightly, and by hand | every fuzz target for a few minutes each |
| `release.yml` | a `v*` tag | the installers, on a draft release |
| `image.yml` | a `v*` tag | the server's image for amd64 and arm64, on ghcr.io and Docker Hub |

## Packaging

`scripts/package.nu` (a [Nushell](https://www.nushell.sh) script) builds the installers into `dist/`, each named for the version in `Cargo.toml`:

```bash
nu scripts/package.nu mac
```

```bash
nu scripts/package.nu windows
```

```bash
nu scripts/package.nu linux
```

`all` makes the three from a Mac: the Mac's `.app` (Apple silicon only) and its `.dmg` natively, the Windows `.exe` through cargo-xwin, and the Linux binaries in Docker, against glibc 2.28 through cargo-zigbuild. The Windows installer is NSIS, run in the same Docker image (`packaging/Dockerfile`). The top of the script lists what each one needs.

| File | What |
|---|---|
| `packaging/macos/Info.plist` | the bundle's identity (`dev.qxuken.carnet`), version, icon and the folder prompts' wording |
| `packaging/windows/carnet.rc` | the `.exe`'s icon and version, compiled in by `build.rs` |
| `packaging/windows/carnet.nsi` | the per-user installer and its uninstaller |
| `packaging/linux/` | the menu entry and `install.sh` that go in the tarball |
| `packaging/build-env.sh` | the Linux and Windows build environment, for the Docker image and CI alike |
| `packaging/server/Dockerfile` | the server's container: one binary, one volume, the web app |
| `packaging/monitoring/` | the server's Grafana dashboard and alert rules, and Prometheus' and Alloy's configs to feed them; its README is the setup |
| `.cargo/config.toml` | the C runtime linked into the `.exe`, so it needs no Visual C++ Redistributable |

The Mac build is signed ad hoc unless `CARNET_SIGN_IDENTITY` names a Developer ID; with `CARNET_NOTARY_PROFILE` too (a `xcrun notarytool store-credentials` profile) the `.dmg` is notarized and stapled, and Gatekeeper lets it straight through.

## Releasing

Releases are made on GitHub, on runners for each system. Tag the version `Cargo.toml` names (`v0.1.0` for 0.1.0) and push it with `main`:

```bash
git push --atomic origin main v0.1.0
```

The tag runs two workflows:

- `release.yml` checks that the tag matches `Cargo.toml`'s version and runs the tests, then builds the Linux tarballs (x64 and arm64) and the Windows installer and `.zip` in `packaging/build-env.sh`'s environment, and the `.dmg` on a macOS runner. It puts them, with `SHA256SUMS`, on a draft release, which is published by hand. The `.app` is signed ad hoc unless the signing secrets the workflow lists exist.
- `image.yml` builds the server's image for amd64 and arm64 and pushes it to `ghcr.io/qxuken/carnet-server` and, with its secrets, to Docker Hub as `qxuken/carnet-server`, tagged with the version, `<major>.<minor>` and `latest` (a prerelease gets only its own tag).

## Licence

Carnet is under either of the Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE)) or the MIT License ([LICENSE-MIT](LICENSE-MIT)), at your option. A contribution you submit for inclusion is licensed the same way, as defined in the Apache-2.0 licence, with no additional terms or conditions.
