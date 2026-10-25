//! The app's half of the automation bridge (task 160, ADR 0015): off unless
//! asked for, a loopback listener whose requests are marshalled onto the
//! UI thread and run through the same handlers real input reaches.
//!
//! * **Enabling.** `aurora-app --mcp-bridge` (or `AURORA_MCP_BRIDGE=1`);
//!   `--mcp-bridge-read-root DIR` / `--mcp-bridge-write-root DIR` (repeatable;
//!   `--mcp-bridge-root` is the read-only shorthand; `AURORA_MCP_BRIDGE_ROOTS`, a
//!   path list) names the only directories the bridge's file tools may
//!   read or write, none = no file tools. Nothing listens and no file is
//!   written otherwise ([`BridgeOptions::from_args`]).
//! * **Threads.** The listener's connection thread waits (with a timeout)
//!   for the UI thread's answer; the UI thread never waits. A request is
//!   put on a bounded queue ([`QUEUE_CAPACITY`]) and `about_to_wait` runs
//!   at most [`JOBS_PER_TURN`] of them per loop turn. A screenshot or an
//!   open finishes on a later turn.
//! * **Policy** (`App::bridge_dispatch`): while a modal dialog (or the quit
//!   review) is open, only reads and `ui.dialog` run; with the command
//!   palette open, commands and file tools are refused; `app.quit`, the
//!   native-dialog commands and any command with a file parameter are never
//!   run over the bridge; nothing here closes the window.
//! * **Indication.** While a controller is connected the window title ends
//!   with [`TITLE_SUFFIX`] and the status bar says so
//!   ([`AUTOMATION_CONNECTED`]); `automation.disconnect` (palette) kicks
//!   the controller and turns the bridge off.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use aurora_automation::Roots;
use aurora_automation::bridge::discovery::{self, Discovery, Published};
use aurora_automation::bridge::protocol::{CallError, Request};
use aurora_automation::bridge::server::{BridgeServer, Dispatcher, ServerConfig};
use aurora_automation::bridge::token;
use aurora_automation::scratch::Scratch;
use serde_json::{Value, json};
use winit::event_loop::EventLoopProxy;

use crate::{App, AppEvent};

/// The palette command that disconnects the controller and disables the
/// bridge.
pub(crate) const COMMAND_DISCONNECT: &str = "automation.disconnect";
/// Appended to the window title while a controller is connected.
pub(crate) const TITLE_SUFFIX: &str = " [automation connected]";
/// Requests waiting for the UI thread; a full queue is refused.
pub(crate) const QUEUE_CAPACITY: usize = 16;
/// Requests the UI thread runs in one loop turn.
pub(crate) const JOBS_PER_TURN: usize = 4;
/// How long a connection thread waits for the UI thread.
pub(crate) const CALL_TIMEOUT: Duration = Duration::from_mins(1);
/// A pending screenshot or open that has not finished after this is failed.
pub(crate) const ASYNC_TIMEOUT: Duration = Duration::from_secs(50);

/// Whether a controller is connected (read by the status bar and title).
pub static AUTOMATION_CONNECTED: AtomicBool = AtomicBool::new(false);

/// What the command line and environment ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeOptions {
    pub enabled: bool,
    /// Directories the file tools may READ from (`--mcp-bridge-root` is a
    /// shorthand for `--mcp-bridge-read-root`).
    pub read_roots: Vec<PathBuf>,
    /// Directories the file tools may create NEW files in; writing needs
    /// `--mcp-bridge-write-root` explicitly.
    pub write_roots: Vec<PathBuf>,
    /// Largest canvas the bridge composites for a preview or an export, in
    /// pixels (the work runs on the UI thread).
    pub max_pixels: u64,
}

impl Default for BridgeOptions {
    fn default() -> Self {
        Self {
            enabled: false,
            read_roots: Vec::new(),
            write_roots: Vec::new(),
            max_pixels: 50_000_000,
        }
    }
}

impl BridgeOptions {
    /// Reads the options from `args` (without the program name) and an
    /// environment lookup. Off unless `--mcp-bridge` is given or
    /// `AURORA_MCP_BRIDGE` is `1`/`true`/`on`/`yes`. Arguments it does not
    /// know are ignored (a platform launcher may add its own).
    ///
    /// # Errors
    /// A flag that needs a value has none, or a value is not usable.
    pub fn from_args(
        args: &[String],
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, String> {
        let mut options = Self {
            enabled: env("AURORA_MCP_BRIDGE").is_some_and(|v| {
                matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "on" | "yes"
                )
            }),
            ..Self::default()
        };
        if let Some(list) = env("AURORA_MCP_BRIDGE_ROOTS").filter(|v| !v.is_empty()) {
            options.read_roots.extend(std::env::split_paths(&list));
        }
        if let Some(list) = env("AURORA_MCP_BRIDGE_WRITE_ROOTS").filter(|v| !v.is_empty()) {
            options.write_roots.extend(std::env::split_paths(&list));
        }
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--mcp-bridge" => options.enabled = true,
                flag @ ("--mcp-bridge-root" | "--mcp-bridge-read-root") => {
                    let value = it
                        .next()
                        .ok_or_else(|| format!("{flag} needs a directory"))?;
                    options.read_roots.push(PathBuf::from(value));
                }
                "--mcp-bridge-write-root" => {
                    let value = it
                        .next()
                        .ok_or_else(|| "--mcp-bridge-write-root needs a directory".to_owned())?;
                    options.write_roots.push(PathBuf::from(value));
                }
                "--mcp-bridge-max-mpx" => {
                    let value = it
                        .next()
                        .and_then(|v| v.parse::<u64>().ok())
                        .filter(|n| (1..=1000).contains(n))
                        .ok_or_else(|| "--mcp-bridge-max-mpx needs a number 1..1000".to_owned())?;
                    options.max_pixels = value * 1_000_000;
                }
                _ => {}
            }
        }
        Ok(options)
    }
}

/// One request waiting for the UI thread.
pub(crate) struct Job {
    pub(crate) request: Request,
    reply: mpsc::Sender<Result<Value, CallError>>,
    abandoned: Arc<AtomicBool>,
    started: Arc<AtomicBool>,
}

impl Job {
    pub(crate) fn finish(&self, result: Result<Value, CallError>) {
        let _ = self.reply.send(result);
    }

    pub(crate) fn live(&self) -> bool {
        !self.abandoned.load(Ordering::SeqCst)
    }

    pub(crate) fn start(&self) {
        self.started.store(true, Ordering::SeqCst);
    }

    /// Looks up a request parameter.
    pub(crate) fn param(&self, name: &str) -> Option<&Value> {
        self.request.params.get(name).filter(|v| !v.is_null())
    }
}

/// Something that finishes on a later loop turn.
pub(crate) enum Pending {
    Screenshot {
        job: Job,
        max_side: u32,
        since: Instant,
    },
    Open {
        job: Job,
        since: Instant,
    },
}

/// A captured frame.
pub(crate) struct CapturedFrame {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rgba: Vec<u8>,
}

pub(crate) struct AppDispatcher {
    tx: mpsc::SyncSender<Job>,
    /// Wakes the UI thread's event loop.
    wake: Box<dyn Fn() + Send + Sync>,
    wait: Duration,
}

impl AppDispatcher {
    pub(crate) fn new(
        tx: mpsc::SyncSender<Job>,
        wake: Box<dyn Fn() + Send + Sync>,
        wait: Duration,
    ) -> Self {
        Self { tx, wake, wait }
    }

    fn wake(&self) {
        (self.wake)();
    }
}

impl Dispatcher for AppDispatcher {
    fn dispatch(&self, request: Request) -> Result<Value, CallError> {
        let (reply, answer) = mpsc::channel();
        let abandoned = Arc::new(AtomicBool::new(false));
        let started = Arc::new(AtomicBool::new(false));
        let job = Job {
            request,
            reply,
            abandoned: Arc::clone(&abandoned),
            started: Arc::clone(&started),
        };
        match self.tx.try_send(job) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(_)) => {
                return Err(CallError::new(
                    "rate_limited",
                    format!("{QUEUE_CAPACITY} requests are already waiting for the app"),
                ));
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                return Err(CallError::new("unavailable", "the app is shutting down"));
            }
        }
        self.wake();
        if let Ok(result) = answer.recv_timeout(self.wait) {
            return result;
        }
        abandoned.store(true, Ordering::SeqCst);
        if started.load(Ordering::SeqCst) {
            Err(CallError::new(
                "timeout",
                "no reply in time: the app started the request and it may still apply; \
                 check the window before retrying",
            ))
        } else {
            Err(CallError::new(
                "timeout",
                "timeout: not run (the app did not get to the request; it is safe to \
                 send again)",
            ))
        }
    }

    fn controller_connected(&self) {
        AUTOMATION_CONNECTED.store(true, Ordering::SeqCst);
        self.wake();
    }

    fn controller_disconnected(&self) {
        AUTOMATION_CONNECTED.store(false, Ordering::SeqCst);
        self.wake();
    }
}

/// The running bridge, held by [`App`].
pub(crate) struct Bridge {
    server: Option<BridgeServer>,
    published: Option<Published>,
    rx: mpsc::Receiver<Job>,
    pub(crate) shown_connected: bool,
    pub(crate) roots: Roots,
    pub(crate) read_root_paths: Vec<PathBuf>,
    pub(crate) write_root_paths: Vec<PathBuf>,
    pub(crate) scratch: Scratch,
    pub(crate) max_pixels: u64,
    pub(crate) pending: Vec<Pending>,
    pub(crate) captured: Option<CapturedFrame>,
    /// Whether the surface supports reading a frame back.
    pub(crate) capture_supported: bool,
    pub(crate) counter: u64,
    pub(crate) staged: Option<PathBuf>,
    pub(crate) discovery_path: Option<PathBuf>,
}

impl Bridge {
    /// Starts the listener and publishes the discovery file.
    ///
    /// # Errors
    /// Why the bridge could not be enabled (the app then runs without it).
    pub(crate) fn start(
        options: &BridgeOptions,
        proxy: EventLoopProxy<AppEvent>,
    ) -> Result<Self, String> {
        let roots = Roots::open(&options.read_roots, &options.write_roots)
            .map_err(|e| format!("a bridge root can't be opened: {e}"))?;
        let scratch = Scratch::create(None).map_err(|e| format!("bridge scratch: {e}"))?;
        let dir = discovery::default_dir().ok_or_else(|| {
            "this platform has no directory for the bridge discovery file".to_owned()
        })?;
        let token_text = token::generate()?;
        let (tx, rx) = mpsc::sync_channel(QUEUE_CAPACITY);
        let proxy = Mutex::new(proxy);
        let dispatcher = Arc::new(AppDispatcher::new(
            tx,
            Box::new(move || {
                let proxy = proxy.lock().unwrap_or_else(PoisonError::into_inner);
                let _ = proxy.send_event(AppEvent::Bridge);
            }),
            CALL_TIMEOUT,
        ));
        let config = ServerConfig {
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            ..ServerConfig::default()
        };
        let server = BridgeServer::start(config, token_text.clone(), dispatcher)
            .map_err(|e| format!("couldn't listen: {e}"))?;
        let record = Discovery::new(
            server.local_addr().port(),
            token_text,
            env!("CARGO_PKG_VERSION"),
        );
        let published = discovery::publish(&dir, &record).map_err(|e| {
            format!(
                "couldn't publish the discovery file in {}: {e}",
                dir.display()
            )
        })?;
        tracing::info!(
            port = record.port,
            file = %published.path().display(),
            read_roots = options.read_roots.len(),
            write_roots = options.write_roots.len(),
            "automation bridge enabled (loopback only)"
        );
        let discovery_path = Some(published.path().to_path_buf());
        Ok(Self {
            server: Some(server),
            published: Some(published),
            rx,
            shown_connected: false,
            roots,
            read_root_paths: options.read_roots.clone(),
            write_root_paths: options.write_roots.clone(),
            scratch,
            max_pixels: options.max_pixels,
            pending: Vec::new(),
            captured: None,
            capture_supported: false,
            counter: 1,
            staged: None,
            discovery_path,
        })
    }

    /// A screenshot is waiting for the next frame.
    pub(crate) fn wants_capture(&self) -> bool {
        self.capture_supported
            && self
                .pending
                .iter()
                .any(|p| matches!(p, Pending::Screenshot { .. }))
    }

    pub(crate) fn next_counter(&mut self) -> u64 {
        self.counter += 1;
        self.counter
    }

    /// Kicks the controller and turns the bridge off for good: closes the
    /// listener (the token stops working) and removes the discovery file.
    pub(crate) fn disable(&mut self) {
        if let Some(mut server) = self.server.take() {
            server.shutdown();
        }
        if let Some(mut published) = self.published.take() {
            published.remove();
        }
        for pending in self.pending.drain(..) {
            let job = match &pending {
                Pending::Screenshot { job, .. } | Pending::Open { job, .. } => job,
            };
            job.finish(Err(CallError::new(
                "disconnected",
                "the bridge was disabled",
            )));
        }
        AUTOMATION_CONNECTED.store(false, Ordering::SeqCst);
    }

    pub(crate) fn take_jobs(&self, max: usize) -> Vec<Job> {
        let mut jobs = Vec::new();
        while jobs.len() < max {
            match self.rx.try_recv() {
                Ok(job) => jobs.push(job),
                Err(_) => break,
            }
        }
        jobs
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.disable();
        if let Some(staged) = self.staged.take() {
            let _ = std::fs::remove_dir_all(staged);
        }
    }
}

/// Why a request is not allowed right now, if it is not.
///
/// `modal` describes an open dialog or quit review, `palette` is whether
/// the command palette is open.
pub(crate) fn policy(
    method: &str,
    action: Option<&str>,
    modal: Option<&str>,
    palette: bool,
) -> Result<(), CallError> {
    const ALWAYS: [&str; 8] = [
        "bridge.status",
        "command.context",
        "doc.inspect",
        "doc.preview",
        "doc.psd_check",
        "ui.inspect",
        "ui.screenshot",
        "ui.dialog",
    ];
    if ALWAYS.contains(&method) {
        return Ok(());
    }
    if method == "ui.menu" && action == Some("list") {
        return Ok(());
    }
    if let Some(modal) = modal {
        return Err(CallError::new(
            "modal_dialog",
            format!("a dialog is open ({modal}); answer it with ui_dialog first"),
        ));
    }
    let needs_no_palette = matches!(method, "command.run" | "doc.open" | "doc.save" | "ui.menu");
    if palette && needs_no_palette {
        return Err(CallError::new(
            "palette_open",
            "the command palette is open; close it with ui_key Escape first",
        ));
    }
    Ok(())
}

thread_local! {
    /// How many bridge jobs are executing on this (the UI) thread right now.
    /// Everything a job does is **synthetic input**: it goes through the
    /// same handlers a real event reaches, so the few actions that must stay
    /// a person's (quitting, switching the bridge off, blocking native file
    /// dialogs) are refused at their one central place by [`gate`], whichever
    /// route (menu bar, palette, key chord, command) reached them. A real
    /// user's input is never inside a job, so it is unaffected even while a
    /// controller is connected.
    static SYNTHETIC_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Marks the current thread as running a controller's job for its lifetime.
pub(crate) struct SyntheticScope;

impl SyntheticScope {
    pub(crate) fn enter() -> Self {
        SYNTHETIC_DEPTH.with(|d| d.set(d.get().saturating_add(1)));
        Self
    }
}

impl Drop for SyntheticScope {
    fn drop(&mut self) {
        SYNTHETIC_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

/// Whether the code running now was driven by a controller's job.
pub(crate) fn synthetic_input() -> bool {
    SYNTHETIC_DEPTH.with(|d| d.get() > 0)
}

/// What a controller's synthetic input may never do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Gated {
    /// Quit Aurora (the menu bar item, a quit request, any exit path).
    Quit,
    /// Switch the bridge off (the person's lever).
    DisconnectAutomation,
    /// A native file dialog: it blocks the UI thread until a person answers.
    NativeDialog,
}

/// The central gate: `Err` with the calm notice text while a controller's job
/// is running. Quitting, disconnecting and native dialogs call it first.
pub(crate) fn gate(action: Gated) -> Result<(), &'static str> {
    if !synthetic_input() {
        return Ok(());
    }
    Err(match action {
        Gated::Quit => "A controller can't quit Aurora. Quitting is for the person at the window.",
        Gated::DisconnectAutomation => {
            "A controller can't switch the bridge off. Use Disconnect Automation in the palette."
        }
        Gated::NativeDialog => {
            "A controller can't open a file dialog. Use doc_open and doc_save with the bridge's roots."
        }
    })
}

/// Ids never run over the bridge: quitting is not a tool.
pub(crate) const BLOCKED_COMMANDS: &[&str] = &["app.quit", COMMAND_DISCONNECT];

impl App {
    /// Starts the bridge when the options ask for it. A failure is logged
    /// and the app runs without it.
    pub(crate) fn enable_bridge(&mut self, options: &BridgeOptions) {
        if !options.enabled {
            return;
        }
        match Bridge::start(options, self.proxy.clone()) {
            Ok(bridge) => {
                eprintln!(
                    "aurora: automation bridge enabled; discovery file {}",
                    bridge
                        .discovery_path
                        .as_deref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default()
                );
                self.bridge = Some(bridge);
            }
            Err(error) => {
                tracing::error!(%error, "the automation bridge could not be enabled");
                eprintln!("aurora: the automation bridge could not be enabled: {error}");
            }
        }
    }

    /// `automation.disconnect`: kicks the controller, turns the bridge off.
    pub(crate) fn disconnect_automation(&mut self) {
        if let Some(bridge) = self.bridge.as_mut() {
            tracing::info!("automation disconnected and disabled by the user");
            bridge.disable();
        }
        self.sync_automation_indicator();
    }

    /// Per loop turn: the connection indicator, the queued requests and the
    /// ones waiting for a later turn.
    pub(crate) fn drain_bridge(&mut self) {
        if self.bridge.is_none() {
            return;
        }
        self.sync_automation_indicator();
        let jobs = self
            .bridge
            .as_ref()
            .map(|b| b.take_jobs(JOBS_PER_TURN))
            .unwrap_or_default();
        for job in jobs {
            if !job.live() {
                continue;
            }
            job.start();
            tracing::info!(method = %job.request.method, "bridge call");
            if let Some(done) = self.bridge_dispatch(job) {
                done.0.finish(done.1);
            }
        }
        self.settle_pending();
    }

    /// Shows or hides the "automation connected" indication.
    pub(crate) fn sync_automation_indicator(&mut self) {
        let connected = AUTOMATION_CONNECTED.load(Ordering::SeqCst);
        let shown = self.bridge.as_ref().is_some_and(|b| b.shown_connected);
        if connected == shown {
            return;
        }
        if let Some(bridge) = self.bridge.as_mut() {
            bridge.shown_connected = connected;
        }
        self.apply_window_title();
        self.sync_status_bar_now();
        self.needs_redraw = true;
    }

    /// Describes the open modal dialog or quit review, if any.
    pub(crate) fn bridge_modal(&self) -> Option<String> {
        if let Some(dialog) = self.dialog.as_ref() {
            let title = self
                .workspace
                .tree
                .accessibility(dialog.root)
                .and_then(|n| n.label())
                .unwrap_or("dialog");
            return Some(title.to_owned());
        }
        self.quit_review.as_ref().map(|_| "quit review".to_owned())
    }

    /// What a controller is told when its call left a dialog open.
    pub(crate) fn bridge_modal_note(&self) -> Option<String> {
        self.bridge_modal().map(|modal| {
            format!(
                "a dialog is open ({modal}); a controller can only acknowledge it or cancel \
                 (ui_dialog); a person answers questions that discard changes or quit"
            )
        })
    }

    /// Runs one request; `Some` is the finished answer, `None` means the
    /// job was stored to finish on a later turn.
    fn bridge_dispatch(&mut self, job: Job) -> Option<(Job, Result<Value, CallError>)> {
        // Everything below is a controller's synthetic input: the central
        // gate ([`gate`]) refuses quit, disconnect and native dialogs until
        // this scope ends.
        let _synthetic = SyntheticScope::enter();
        let method = job.request.method.clone();
        let action = job
            .param("action")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let modal = self.bridge_modal();
        let palette = self.command_palette.is_some();
        if let Err(error) = policy(&method, action.as_deref(), modal.as_deref(), palette) {
            return Some((job, Err(error)));
        }
        let outcome = match method.as_str() {
            "bridge.status" => Ok(self.bridge_status()),
            "command.context" => Ok(self.bridge_command_context()),
            "command.run" => self.bridge_command_run(&job),
            "doc.inspect" => Ok(self.bridge_doc_inspect()),
            "doc.preview" => self.bridge_doc_preview(&job),
            "doc.psd_check" => self.bridge_doc_psd_check(),
            "doc.save" => self.bridge_doc_save(&job),
            "doc.open" => return self.bridge_doc_open(job),
            "ui.inspect" => self.bridge_ui_inspect(&job),
            "ui.screenshot" => return self.bridge_ui_screenshot(job),
            "ui.pointer" => self.bridge_ui_pointer(&job),
            "ui.key" => self.bridge_ui_key(&job),
            "ui.menu" => self.bridge_ui_menu(&job),
            "ui.dialog" => self.bridge_ui_dialog(&job),
            other => Err(CallError::new(
                "unknown_method",
                format!("unknown method `{other}`"),
            )),
        };
        Some((job, outcome))
    }

    fn bridge_status(&self) -> Value {
        let (width, height) = self
            .window
            .as_ref()
            .map(|w| {
                let s = w.inner_size();
                (s.width, s.height)
            })
            .unwrap_or_default();
        json!({
            "app_version": env!("CARGO_PKG_VERSION"),
            "pid": std::process::id(),
            "window": {"physical_width": width, "physical_height": height, "scale_factor": self.scale_factor},
            "read_roots": self.bridge.as_ref().map(|b| b.read_root_paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>()),
            "write_roots": self.bridge.as_ref().map(|b| b.write_root_paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>()),
            "modal": self.bridge_modal(),
            "palette_open": self.command_palette.is_some(),
            "screenshots": self.bridge.as_ref().is_some_and(|b| b.capture_supported),
        })
    }

    /// Finishes screenshots whose frame arrived and opens that completed;
    /// fails ones that waited too long.
    fn settle_pending(&mut self) {
        let Some(bridge) = self.bridge.as_mut() else {
            return;
        };
        if bridge.pending.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut bridge.pending);
        let mut keep = Vec::new();
        for item in pending {
            match item {
                Pending::Screenshot {
                    job,
                    max_side,
                    since,
                } => {
                    if !job.live() {
                        continue;
                    }
                    let frame = self.bridge.as_mut().and_then(|b| b.captured.take());
                    if let Some(frame) = frame {
                        job.finish(crate::bridge_ui::encode_screenshot(
                            &frame,
                            max_side,
                            self.scale_factor,
                        ));
                    } else if since.elapsed() > ASYNC_TIMEOUT {
                        job.finish(Err(CallError::new(
                            "timeout",
                            "no frame was drawn (is the window hidden, minimised or occluded?)",
                        )));
                    } else {
                        keep.push(Pending::Screenshot {
                            job,
                            max_side,
                            since,
                        });
                    }
                }
                Pending::Open { job, since } => {
                    if !job.live() {
                        continue;
                    }
                    if self.open_worker.pending_path().is_some() {
                        if since.elapsed() > ASYNC_TIMEOUT {
                            job.finish(Err(CallError::new(
                                "timeout",
                                "the document is still opening; check the window",
                            )));
                        } else {
                            keep.push(Pending::Open { job, since });
                        }
                    } else if self
                        .dialog
                        .as_ref()
                        .is_some_and(|d| d.purpose == crate::DialogPurpose::OpenFailed)
                    {
                        let modal = self.bridge_modal().unwrap_or_default();
                        job.finish(Err(CallError::new(
                            "failed",
                            format!("the file could not be opened ({modal}); read the dialog with ui_dialog"),
                        )));
                    } else {
                        let mut answer = self.bridge_doc_inspect();
                        if let (Some(modal), Some(object)) =
                            (self.bridge_modal(), answer.as_object_mut())
                        {
                            object.insert("dialog".into(), json!(modal));
                        }
                        job.finish(Ok(answer));
                    }
                }
            }
        }
        if let Some(bridge) = self.bridge.as_mut() {
            bridge.pending.extend(keep);
            if !bridge.pending.is_empty() {
                self.needs_redraw = true;
            }
        }
    }
}
