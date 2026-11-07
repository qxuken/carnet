//! A disposable note driven on the production native run loop. PumpRunner
//! intentionally disables the macOS display-link pacer, so must not be used.

use super::*;
use kui_native::{InputEvent, KeyMods, KeyPress};
use serde_json::json;
use std::cell::RefCell;
use std::fmt::Write;
use std::io::Write as _;
use std::path::{Path, PathBuf};

struct Sample {
    phase: &'static str,
    frame: u64,
    at_ms: f64,
    started_ms: f64,
    timing: kui_native::FrameSample,
}

const PHASES: [&str; 4] = ["warmup", "scrolling", "typing warmup", "typing"];

fn requested_thread_qos() -> Option<u32> {
    #[cfg(target_os = "macos")]
    {
        unsafe extern "C" { fn qos_class_self() -> u32; }
        // Reads the calling thread's requested class; it does not change
        // scheduling or include temporary priority inheritance.
        Some(unsafe { qos_class_self() })
    }
    #[cfg(not(target_os = "macos"))]
    { None }
}

struct NativePerf {
    app: Carnet,
    source: PathBuf,
    output: PathBuf,
    mode: String,
    seconds: f64,
    tables: usize,
    largest: Slot,
    wall: f64,
    epoch: Instant,
    phase: usize,
    phase_end: Instant,
    idle_check: bool,
    idle_started_ms: Option<f64>,
    waker: Option<Waker>,
    seen: u64,
    starts: Vec<f64>,
    previous_phase: &'static str,
    samples: Vec<Sample>,
    edits: usize,
    inputs: usize,
    direction: f32,
    focused_frames: [usize; 4],
    phase_frames: [usize; 4],
    scale: f32,
    refresh: Option<f32>,
    finished: bool,
    error: Option<String>,
    outcome: Rc<RefCell<Option<Result<(), String>>>>,
}

impl NativePerf {
    fn phase_name(&self) -> &'static str {
        if self.idle_check && self.phase == 2 { "scroll idle" } else { PHASES[self.phase] }
    }

    fn input(&mut self, core: &mut Core, event: InputEvent) {
        // The runner times this hook once as input work.
        for event in core.handle_input(event) { self.app.on_event_with(event, core); }
        self.app.after_input(core);
    }

    fn drive(&mut self, core: &mut Core) {
        let now = Instant::now();
        let total = core.stats.total;
        if total > self.seen {
            let count = (total - self.seen) as usize;
            for (index, timing) in core.stats.iter().skip(core.stats.len().saturating_sub(count)).enumerate() {
                self.samples.push(Sample { phase: self.previous_phase, frame: self.seen + index as u64 + 1,
                    at_ms: now.duration_since(self.epoch).as_secs_f64() * 1000.0,
                    started_ms: self.starts.get((self.seen + index as u64) as usize).copied().unwrap_or(f64::NAN), timing });
            }
            self.seen = total;
        }
        self.starts.push(now.duration_since(self.epoch).as_secs_f64() * 1000.0);
        if self.finished {
            core.push_window_command(kui_native::WindowCommand::Close(kui_native::WindowId::MAIN));
            return;
        }
        if now >= self.phase_end {
            self.phase += 1;
            if self.phase == PHASES.len() || (self.idle_check && self.phase == 3) {
                self.finished = true;
                core.push_window_command(kui_native::WindowCommand::Close(kui_native::WindowId::MAIN));
                return;
            }
            core.perf_mark(&format!("native perf: {}", self.phase_name()));
            self.phase_end = now + Duration::from_secs_f64(if self.phase == 2 {
                if self.idle_check { 5.0 } else { 1.0 }
            } else { self.seconds });
            if self.idle_check && self.phase == 2 {
                self.idle_started_ms = Some(now.duration_since(self.epoch).as_secs_f64() * 1000.0);
                // One audit wake ends the quiet interval. It must not
                // supply the redraws whose absence this check measures.
                if let Some(waker) = self.waker.clone() {
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_secs(5));
                        waker.wake();
                    });
                }
            }
            if self.phase == 1 {
                if let Some(pane) = self.app.page_rect {
                    self.input(core, InputEvent::CursorMoved(Vec2::new(pane.x + 28.0, pane.y + pane.h / 2.0)));
                }
            } else if self.phase == 2 && !self.idle_check {
                self.app.caret = Caret::at(Pos { slot: self.largest, byte: 0 });
                self.app.editing = true;
                self.app.focus_page = true;
                self.app.reveal = true;
            } else if let Some(key) = self.app.page_key { core.set_focus(Some(key)); }
        }
        self.phase_frames[self.phase] += 1;
        self.focused_frames[self.phase] += usize::from(core.env.focused);
        self.refresh = core.env.refresh_hz;
        if self.phase_frames.iter().sum::<usize>() == 1 { core.focus_window(kui_native::WindowId::MAIN); }
        match self.phase {
            1 => {
                if let Some(g) = self.app.scroll_key.and_then(|key| core.scroll_geometry(key)) {
                    if g.offset.y >= g.max_offset.y - 1.0 { self.direction = 1.0; }
                    if g.offset.y <= 1.0 { self.direction = -1.0; }
                    self.input(core, InputEvent::Scroll(Vec2::new(0.0, self.direction * g.max_offset.y / 600.0)));
                }
            }
            3 => {
                let before = page::text(self.app.doc(), self.app.caret.head.slot).len();
                if self.inputs % 48 < 24 { self.input(core, InputEvent::Commit("x".into())); }
                else {
                    self.input(core, InputEvent::KeyDown(KeyPress::new(KeyCode::Backspace, KeyMods::NONE)));
                    self.input(core, InputEvent::KeyUp(KeyPress::new(KeyCode::Backspace, KeyMods::NONE)));
                }
                self.inputs += 1;
                if page::text(self.app.doc(), self.app.caret.head.slot).len() != before { self.edits += 1; }
                else { self.error = Some(format!("typing failed at operation {}", self.inputs)); }
            }
            _ => {}
        }
        self.previous_phase = self.phase_name();
    }

    fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        if !self.finished { return Err("benchmark window closed before completion".into()); }
        if let Some(error) = &self.error { return Err(error.clone().into()); }
        let idle_frames = self.idle_started_ms.map(|start| self.samples.iter()
            .filter(|s| s.phase == "scroll idle" && s.started_ms >= start + 1000.0 && s.started_ms < start + 4500.0).count());
        // A visible caret may blink twice per second. A continuous render
        // loop produces hundreds of frames in this 3.5-second interval.
        let metadata = json!({
            "source": self.source, "table_mode": self.mode, "tables": self.tables,
            "runner": "Launcher::run (production native event loop and display-link pacer)",
            "window_logical": [1100, 740], "scale": self.scale,
            "display_reported_hz": self.refresh, "start_unix_seconds": self.wall,
            "duration_per_phase_seconds": self.seconds,
            "idle_check": self.idle_check,
            "idle_steady_frames": idle_frames,
            "idle_steady_seconds": if self.idle_check { Some(3.5) } else { None },
            "input": "core dispatch before the app view, one scroll or validated commit/backspace per admitted frame",
            "timing_scope": "native CPU build/layout/encode/submit/acquire; input injection included once in input_ms; GPU completion requires external Metal trace",
            "timestamp_scope": "elapsed_ms samples the previous frame at next entry; frame_started_ms is the labelled frame input-sampling time; physical display events establish presentation",
            "validated_cell_edits": self.edits, "typing_operations": self.inputs,
            "focused_frames_by_phase": self.focused_frames, "frames_by_phase": self.phase_frames,
            "launcher_frame_latency": crate::NATIVE_FRAME_LATENCY,
            "frame_latency_override": std::env::var("KUI_FRAME_LATENCY").ok(),
            "frame_pacing_override": std::env::var("KUI_FRAME_PACING").ok(),
            "native_window_id": kui_native::WindowId::MAIN.0,
            "requested_main_thread_qos": requested_thread_qos(),
            "requested_window_backdrop": if self.app.settings.translucent { "translucent" } else { "solid" },
        });
        let mut csv = String::from("phase,frame,elapsed_ms,frame_started_ms,input_ms,view_ms,layout_ms,render_submit_ms,maintenance_ms,acquire_wait_ms,cpu_work_ms,cpu_total_ms\n");
        for s in &self.samples {
            let t = s.timing;
            writeln!(&mut csv, "{},{},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6}", s.phase, s.frame, s.at_ms, s.started_ms, t.input_ms, t.view_ms, t.layout_ms, t.render_ms, t.maintenance_ms, t.wait_ms, t.work(), t.total())?;
        }
        std::fs::File::create_new(self.output.join("frames.csv"))?.write_all(csv.as_bytes())?;
        std::fs::File::create_new(self.output.join("metadata.json"))?
            .write_all(&serde_json::to_vec_pretty(&metadata)?)?;
        if idle_frames.is_some_and(|count| count > 12) {
            return Err(format!("scrolling stopped but {} frames rendered during 3.5 seconds of idle", idle_frames.unwrap()).into());
        }
        eprintln!("native perf: completed; {} validated edits", self.edits);
        Ok(())
    }
}

impl App for NativePerf {
    fn after_input(&mut self, core: &mut Core) { self.app.after_input(core); }
    fn setup(&mut self, waker: Waker) { self.waker = Some(waker.clone()); self.app.setup(waker); }
    fn on_wake(&mut self) { self.app.on_wake(); }
    fn before_frame(&mut self, core: &mut Core) { if core.env.window.id == kui_native::WindowId::MAIN { self.drive(core); } }
    fn view(&mut self, ui: &mut Ui<'_>) {
        self.scale = ui.scale();
        self.app.view(ui);
        if !self.finished && !(self.idle_check && self.phase == 2) { ui.request_frame(); }
    }
    fn on_event_with(&mut self, event: UiEvent, core: &mut Core) { self.app.on_event_with(event, core); }
    fn on_event(&mut self, event: UiEvent) { self.app.on_event(event); }
    fn teardown(&mut self) {
        self.app.teardown();
        *self.outcome.borrow_mut() = Some(self.save().map_err(|error| error.to_string()));
    }
}

pub fn run(note: &Path, output: &Path, mode: &str, seconds: f64) -> Result<(), Box<dyn std::error::Error>> {
    if !seconds.is_finite() || !(1.0..=120.0).contains(&seconds) { return Err("seconds must be between 1 and 120".into()); }
    let table_width = crate::settings::TableWidth::from_key(mode).ok_or("table mode must be page, window or scroll")?;
    let source = std::fs::read_to_string(note)?;
    std::fs::create_dir_all(output)?;
    for name in ["frames.csv", "metadata.json"] {
        let path = output.join(name);
        if path.symlink_metadata().is_ok() {
            return Err(format!("capture output already exists: {}", path.display()).into());
        }
    }
    let library = output.join(format!("library-{}", std::process::id()));
    std::fs::create_dir(&library)?;
    let store = Store::open(library)?;
    let translucent = match std::env::var("CARNET_PERF_WINDOW").as_deref() {
        Ok("solid") => false,
        Ok("translucent") | Err(_) => true,
        _ => return Err("CARNET_PERF_WINDOW must be solid or translucent".into()),
    };
    let backdrop = if translucent { kui_native::Backdrop::Blur } else { kui_native::Backdrop::Opaque };
    let settings = Settings { table_width, translucent, check_updates: false, ..Settings::default() };
    let mut app = Carnet::with_settings(store, settings);
    app.updates = crate::update::Updates::with(None);
    let idle_check = std::env::var("CARNET_PERF_IDLE_CHECK").as_deref() == Ok("1");
    app.hud = idle_check;
    app.new_note();
    let (title, doc) = carnet_core::doc::from_markdown(&source);
    app.note().doc = doc;
    if let Some(title) = title { app.note().title = title; }
    app.edited();
    app.caret = Caret::at(page::first(app.doc()));
    app.focus_page = true;
    app.focus_req = None;
    let tables = app.doc().blocks().iter().filter(|b| matches!(b.kind, Kind::Table(_))).count();
    let largest = app.doc().blocks().iter().filter_map(|b| match &b.kind {
        Kind::Table(t) => Some((t.rows.len(), Slot { block: b.id, cell: Some((t.rows[t.body().min(t.rows.len() - 1)].id, t.cols[0])) })),
        _ => None,
    }).max_by_key(|(rows, _)| *rows).map(|(_, slot)| slot).ok_or("note has no tables")?;
    let outcome = Rc::new(RefCell::new(None));
    let epoch = Instant::now();
    let driver = NativePerf { app, source: note.into(), output: output.into(), mode: mode.into(), seconds, tables, largest,
        wall: SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs_f64(), epoch, phase: 0,
        phase_end: epoch + Duration::from_secs(3), idle_check, idle_started_ms: None, waker: None, seen: 0, starts: Vec::new(), previous_phase: PHASES[0], samples: Vec::new(),
        edits: 0, inputs: 0, direction: -1.0, focused_frames: [0; 4], phase_frames: [0; 4], scale: 1.0, refresh: None,
        finished: false, error: None, outcome: outcome.clone() };
    kui_native::app("Carnet — native frame measurement")
        .frame_latency(crate::NATIVE_FRAME_LATENCY)
        .chrome(kui_native::Chrome::Custom).titlebar(kui_native::Titlebar::Tall)
        .size(1100.0, 740.0).min_size(360.0, 420.0)
        .backdrop(backdrop).diagnostics(false).run(driver)?;
    outcome.take().ok_or("benchmark did not shut down cleanly")?.map_err(Into::into)
}
