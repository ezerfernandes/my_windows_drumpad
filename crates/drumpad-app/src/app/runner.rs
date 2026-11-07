//! Winit event loop, OpenGL surface, and egui integration.
//!
//! This is a direct integration rather than `eframe` for one reason: keyboard
//! events must reach the pad router on the winit event itself, before egui's
//! next frame is built. The shell keeps the GL surface and the egui painter
//! here; all app behaviour lives in [`crate::state`] and [`crate::app::actions`].

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_glow::egui_winit::winit;
use glutin::config::GlConfig as _;
use glutin::display::{GetGlDisplay as _, GlDisplay as _};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{ModifiersState, PhysicalKey};
use winit::raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
use winit::window::WindowId;

use crate::app::actions::{Action, dispatch};
use crate::app::hotkeys::{routes_to_pads, shortcut};
use crate::app::kits::KitService;
use crate::state::keymap::KeyInput;
use crate::state::notifications::{DialogAction, DialogIntent};
use crate::state::prefs::UiPreferences;
use crate::state::{AppState, AudioStatus, Effect};
use crate::ui::{self, UiRoot};

use drumpad_audio::{AudioSystemEvent, MvpAudioRuntime};

/// Target frame interval. The stage UI runs at a steady 60 fps even when idle,
/// because a pad hit must light up on the very next frame.
pub const FRAME_INTERVAL: Duration = Duration::from_micros(16_666);

/// Largest delta the animation clocks accept after a stall or a debugger pause.
const MAX_FRAME_DELTA: Duration = Duration::from_millis(250);

/// Everything the shell needs to start.
#[derive(Debug, Clone)]
pub struct AppOptions {
    /// Resolved app directories.
    pub paths: crate::settings::AppPaths,
    /// Foundation settings (logging level and UI scale).
    pub settings: drumpad_model::config::AppSettings,
    /// UI preferences, loaded from `preferences.json` when possible.
    pub preferences: UiPreferences,
    /// Window title.
    pub title: String,
}

impl AppOptions {
    /// Load UI preferences beside the foundation settings.
    pub fn new(
        paths: crate::settings::AppPaths,
        settings: drumpad_model::config::AppSettings,
    ) -> Self {
        let path = UiPreferences::path_in(paths.config_dir());
        let preferences = UiPreferences::load(&path).unwrap_or_else(|error| {
            tracing::warn!(%error, "could not load UI preferences; using defaults");
            UiPreferences::default()
        });
        Self {
            paths,
            settings,
            preferences,
            title: format!("Drumpad {}", env!("CARGO_PKG_VERSION")),
        }
    }

    /// Where preferences are persisted.
    pub fn preferences_path(&self) -> PathBuf {
        UiPreferences::path_in(self.paths.config_dir())
    }
}

/// Custom event used to wake the loop for an egui repaint request.
#[derive(Debug)]
pub enum UserEvent {
    /// Repaint after the given delay.
    Redraw(Duration),
}

/// Run the app until the window closes.
pub fn run(options: AppOptions) -> drumpad_model::error::Result<()> {
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(|error| ui_error("could not create the window event loop", error))?;
    let proxy = event_loop.create_proxy();
    let mut shell = AppShell::new(options, proxy);
    event_loop
        .run_app(&mut shell)
        .map_err(|error| ui_error("the window event loop stopped unexpectedly", error))?;
    Ok(())
}

/// Discover paths and settings, then run. Logging must already be initialised by
/// the caller ([`crate::app::run`] does not touch it).
pub fn run_default() -> drumpad_model::error::Result<()> {
    let paths = crate::settings::AppPaths::discover()?;
    paths.ensure()?;
    let settings = crate::settings::SettingsStore::new(paths.settings_path()).load_or_create()?;
    run(AppOptions::new(paths, settings))
}

fn ui_error(context: &str, error: impl std::fmt::Display) -> drumpad_model::error::Error {
    drumpad_model::error::Error::io(context, std::io::Error::other(error.to_string()))
}

/// Desired window size before the monitor is consulted. This is exactly the
/// stage design size, so a monitor with room shows the UI at 100%.
const DESIRED_WINDOW: (f64, f64) = (1440.0, 900.0);
/// Smallest window that still shows the full 4x4 grid and the chrome.
const MIN_WINDOW: (f64, f64) = (960.0, 600.0);
/// Fraction of the usable width the initial window may cover.
const WORK_WIDTH_FRACTION: f64 = 0.92;
/// Fraction of the usable height the initial window may cover.
///
/// Deliberately smaller than the width fraction: Windows positions a new window
/// ~8% down the screen by default, and the title bar sits above the client
/// area. Reserving that space is what keeps the status bar on screen.
const WORK_HEIGHT_FRACTION: f64 = 0.84;

/// Base layout size the stage UI is designed for, in logical points.
///
/// The shell, pad grid, inspector, and status bar were composed for a
/// 1440x900 client. Fit zoom is derived from this size, not from 1280x720, so a
/// 1382x777 logical client scales down instead of clipping the bottom row.
pub const DESIGN_WIDTH: f64 = 1440.0;
/// Base layout height the stage UI is designed for, in logical points.
pub const DESIGN_HEIGHT: f64 = 900.0;

/// Initial inner size in logical pixels, clamped to the primary monitor.
///
/// QA found the 1440x900 logical window overflowing a 1536x816 physical work
/// area at 125% scaling. The monitor reports physical pixels, so convert to
/// logical first, take 90% of that as the cap, and never request more than the
/// cap.
fn initial_inner_size(event_loop: &ActiveEventLoop) -> winit::dpi::LogicalSize<f64> {
    let (inner, _) = fitted_window_sizes(primary_work_area(event_loop));
    inner
}

/// Minimum inner size in logical pixels, also clamped to the work area.
fn initial_min_size(event_loop: &ActiveEventLoop) -> winit::dpi::LogicalSize<f64> {
    let (_, min) = fitted_window_sizes(primary_work_area(event_loop));
    min
}

/// Logical work area of the primary monitor, falling back to a sane default.
///
/// `monitor.size()` is the full monitor. On Windows the real work area (screen
/// minus taskbar) is queried from `SystemParametersInfoW`; elsewhere 90% of the
/// monitor is used as a conservative stand-in.
fn primary_work_area(event_loop: &ActiveEventLoop) -> (f64, f64) {
    let monitor = event_loop
        .primary_monitor()
        .or_else(|| event_loop.available_monitors().next());
    let Some(monitor) = monitor else {
        return DESIRED_WINDOW;
    };
    let scale = monitor.scale_factor().max(0.1);

    #[cfg(windows)]
    if let Some((width, height)) = windows_work_area_physical() {
        return (width as f64 / scale, height as f64 / scale);
    }

    let physical = monitor.size();
    (
        physical.width as f64 / scale,
        physical.height as f64 / scale,
    )
}

/// Physical work area of the primary monitor, excluding the taskbar.
#[cfg(windows)]
fn windows_work_area_physical() -> Option<(u32, u32)> {
    #[repr(C)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn SystemParametersInfoW(
            action: u32,
            param: u32,
            data: *mut core::ffi::c_void,
            win_ini: u32,
        ) -> i32;
    }

    const SPI_GETWORKAREA: u32 = 0x0030;
    let mut rect = Rect {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: `rect` is a valid, properly aligned RECT for the duration of the
    // call, and SPI_GETWORKAREA writes exactly one RECT.
    let ok = unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, (&raw mut rect).cast(), 0) };
    if ok == 0 {
        return None;
    }
    let width = (rect.right - rect.left).max(0);
    let height = (rect.bottom - rect.top).max(0);
    (width > 0 && height > 0).then_some((width as u32, height as u32))
}

/// Clamp the desired size against a logical work area.
///
/// Returns `(inner, min)` where `min <= inner` always holds, so winit cannot
/// reject the pair, and both fit inside the work area.
fn fitted_window_sizes(
    work_area: (f64, f64),
) -> (winit::dpi::LogicalSize<f64>, winit::dpi::LogicalSize<f64>) {
    let (desired_width, desired_height) = DESIRED_WINDOW;
    let (min_width, min_height) = MIN_WINDOW;
    let cap_width = (work_area.0 * WORK_WIDTH_FRACTION).max(320.0);
    let cap_height = (work_area.1 * WORK_HEIGHT_FRACTION).max(240.0);

    let min_width = min_width.min(cap_width);
    let min_height = min_height.min(cap_height);
    let inner_width = desired_width.min(cap_width).max(min_width);
    let inner_height = desired_height.min(cap_height).max(min_height);

    (
        winit::dpi::LogicalSize::new(inner_width, inner_height),
        winit::dpi::LogicalSize::new(min_width, min_height),
    )
}

/// Zoom applied to the egui UI so the whole 1280x720 stage layout fits.
///
/// A window that is physically large can still be below 1280x720 in logical
/// points once Windows scaling is applied. Egui zoom scales points, so the
/// effective canvas becomes `inner_logical / zoom`; choosing the zoom from the
/// smaller axis guarantees at least a 1280x720 layout area. Zoom never enlarges
/// the UI beyond 1.0.
pub fn fit_zoom_for(inner_logical: (f64, f64)) -> f32 {
    if inner_logical.0 <= 0.0 || inner_logical.1 <= 0.0 {
        return 1.0;
    }
    let zoom = (inner_logical.0 / DESIGN_WIDTH).min(inner_logical.1 / DESIGN_HEIGHT);
    zoom.clamp(0.5, 1.0) as f32
}

struct GlWindow {
    window: winit::window::Window,
    gl_context: glutin::context::PossiblyCurrentContext,
    gl_display: glutin::display::Display,
    gl_surface: glutin::surface::Surface<glutin::surface::WindowSurface>,
}

impl GlWindow {
    fn resize(&self, size: winit::dpi::PhysicalSize<u32>) {
        use glutin::surface::GlSurface as _;
        let width = NonZeroU32::new(size.width.max(1)).expect("non-zero");
        let height = NonZeroU32::new(size.height.max(1)).expect("non-zero");
        self.gl_surface.resize(&self.gl_context, width, height);
    }

    fn swap_buffers(&self) -> glutin::error::Result<()> {
        use glutin::surface::GlSurface as _;
        self.gl_surface.swap_buffers(&self.gl_context)
    }

    fn get_proc_address(&self, address: &std::ffi::CStr) -> *const std::ffi::c_void {
        use glutin::display::GlDisplay as _;
        self.gl_display.get_proc_address(address)
    }
}

struct AppShell {
    state: AppState,
    ui: UiRoot,
    options: AppOptions,
    proxy: EventLoopProxy<UserEvent>,
    gl_window: Option<GlWindow>,
    egui: Option<egui_glow::EguiGlow>,
    repaint_delay: Duration,
    last_frame: Instant,
    modifiers: ModifiersState,
    pending_drop: Vec<PathBuf>,
    queued_actions: Vec<Action>,
    gl: Option<Arc<glow::Context>>,
    audio: Option<MvpAudioRuntime>,
    kits: Option<KitService>,
    audio_attempted: bool,
    audio_error: Option<String>,
    last_audio_poll: Instant,
    last_fit_zoom: f32,
}

impl AppShell {
    fn new(options: AppOptions, proxy: EventLoopProxy<UserEvent>) -> Self {
        let mut state = AppState::new(options.preferences.clone());
        let ui = UiRoot::new(&options.preferences);

        // Open the durable kit library. It installs the demo kit on first run
        // and loads the active kit, so the grid shows real pad names/samples
        // immediately instead of the empty placeholder catalogue.
        let kits = match KitService::open(options.paths.data_dir().join("kits")) {
            Ok(mut service) => {
                if let Err(error) = service.refresh(&mut state.kits) {
                    tracing::error!(%error, "could not list the kit library");
                    state.notifications.error(
                        "Kit library could not be read",
                        error,
                        DialogAction::new(DialogIntent::OpenLogFolder),
                    );
                }
                Some(service)
            }
            Err(error) => {
                tracing::error!(%error, "could not open the kit library");
                state.notifications.error(
                    "Kit library could not open",
                    format!("{error}. The demo kit stays playable."),
                    DialogAction::new(DialogIntent::OpenLogFolder),
                );
                None
            }
        };

        Self {
            state,
            ui,
            options,
            proxy,
            gl_window: None,
            egui: None,
            repaint_delay: FRAME_INTERVAL,
            last_frame: Instant::now(),
            modifiers: ModifiersState::empty(),
            pending_drop: Vec::new(),
            queued_actions: Vec::new(),
            gl: None,
            audio: None,
            kits,
            audio_attempted: false,
            audio_error: None,
            last_audio_poll: Instant::now(),
            last_fit_zoom: 1.0,
        }
    }

    fn create_display(&mut self, event_loop: &ActiveEventLoop) {
        let attributes = winit::window::WindowAttributes::default()
            .with_title(self.options.title.clone())
            .with_inner_size(initial_inner_size(event_loop))
            .with_min_inner_size(initial_min_size(event_loop))
            .with_visible(false);

        let template = glutin::config::ConfigTemplateBuilder::new()
            .with_depth_size(0)
            .with_stencil_size(0)
            .with_transparency(false);

        let (window, gl_config) = match glutin_winit::DisplayBuilder::new()
            .with_window_attributes(Some(attributes))
            .build(event_loop, template, |configs| {
                configs
                    .reduce(|best, candidate| {
                        if candidate.num_samples() == 0 && best.num_samples() != 0 {
                            candidate
                        } else {
                            best
                        }
                    })
                    .expect("at least one GL config")
            }) {
            Ok(result) => result,
            Err(error) => {
                tracing::error!(%error, "could not create a GL display");
                self.fail_startup("Could not create an OpenGL display.");
                return;
            }
        };

        let gl_display = gl_config.display();
        let raw_handle: Option<RawWindowHandle> = window
            .as_ref()
            .and_then(|window| window.window_handle().map(|handle| handle.as_raw()).ok());

        let context_attributes = glutin::context::ContextAttributesBuilder::new().build(raw_handle);
        let fallback_attributes = glutin::context::ContextAttributesBuilder::new()
            .with_context_api(glutin::context::ContextApi::Gles(None))
            .build(raw_handle);
        let not_current = unsafe {
            gl_display
                .create_context(&gl_config, &context_attributes)
                .or_else(|_| gl_display.create_context(&gl_config, &fallback_attributes))
        };
        let not_current = match not_current {
            Ok(context) => context,
            Err(error) => {
                tracing::error!(%error, "could not create a GL context");
                self.fail_startup("Could not create an OpenGL context.");
                return;
            }
        };

        let window = match window {
            Some(window) => window,
            None => match glutin_winit::finalize_window(
                event_loop,
                winit::window::WindowAttributes::default()
                    .with_title(self.options.title.clone())
                    .with_visible(false),
                &gl_config,
            ) {
                Ok(window) => window,
                Err(error) => {
                    tracing::error!(%error, "could not finalize the window");
                    self.fail_startup("Could not create the app window.");
                    return;
                }
            },
        };

        let size = window.inner_size();
        let width = NonZeroU32::new(size.width.max(1)).expect("non-zero");
        let height = NonZeroU32::new(size.height.max(1)).expect("non-zero");
        let surface_attributes =
            glutin::surface::SurfaceAttributesBuilder::<glutin::surface::WindowSurface>::new()
                .build(
                    window.window_handle().expect("window handle").as_raw(),
                    width,
                    height,
                );
        let gl_surface = unsafe {
            use glutin::display::GlDisplay as _;
            gl_display.create_window_surface(&gl_config, &surface_attributes)
        };
        let gl_surface = match gl_surface {
            Ok(surface) => surface,
            Err(error) => {
                tracing::error!(%error, "could not create the GL surface");
                self.fail_startup("Could not create an OpenGL surface.");
                return;
            }
        };
        let gl_context = {
            use glutin::context::NotCurrentGlContext as _;
            match not_current.make_current(&gl_surface) {
                Ok(context) => context,
                Err(error) => {
                    tracing::error!(%error, "could not make the GL context current");
                    self.fail_startup("Could not activate the OpenGL context.");
                    return;
                }
            }
        };
        use glutin::surface::GlSurface as _;
        let _ = gl_surface.set_swap_interval(
            &gl_context,
            glutin::surface::SwapInterval::Wait(NonZeroU32::MIN),
        );

        let gl_window = GlWindow {
            window,
            gl_context,
            gl_display,
            gl_surface,
        };
        let size = gl_window.window.inner_size();
        let scale = gl_window.window.scale_factor().max(0.1);
        tracing::info!(
            physical_width = size.width,
            physical_height = size.height,
            scale_factor = scale,
            logical_width = size.width as f64 / scale,
            logical_height = size.height as f64 / scale,
            fit_zoom = fit_zoom_for((size.width as f64 / scale, size.height as f64 / scale)),
            "window created"
        );
        let gl = unsafe {
            glow::Context::from_loader_function(|name| {
                let name = std::ffi::CString::new(name).expect("valid GL name");
                gl_window.get_proc_address(&name)
            })
        };
        let gl = Arc::new(gl);

        let egui = egui_glow::EguiGlow::new(event_loop, gl.clone(), None, None, true);
        let proxy = std::sync::Mutex::new(self.proxy.clone());
        egui.egui_ctx.set_request_repaint_callback(move |info| {
            if let Ok(proxy) = proxy.lock() {
                let _ = proxy.send_event(UserEvent::Redraw(info.delay));
            }
        });
        egui.egui_ctx.set_visuals(egui::Visuals::dark());

        gl_window.window.set_visible(true);
        gl_window.window.request_redraw();
        self.gl_window = Some(gl_window);
        self.egui = Some(egui);
        self.gl = Some(gl);
        self.repaint_delay = FRAME_INTERVAL;
        self.last_frame = Instant::now();
        // Set the zoom from the real client size before the first paint.
        self.update_fit_zoom();
    }

    fn fail_startup(&mut self, message: &str) {
        self.audio_error = Some(message.to_owned());
        self.state.notifications.error(
            "Drumpad could not start",
            message.to_owned(),
            DialogAction::new(DialogIntent::OpenLogFolder),
        );
        tracing::error!("{message}");
    }

    /// Start the shared MVP audio runtime once the window exists.
    ///
    /// A failure is deliberately non-fatal: the window stays open, the status
    /// bar says audio is unavailable, and a dialog offers the next action.
    fn start_audio(&mut self) {
        self.audio_attempted = true;
        match MvpAudioRuntime::start() {
            Ok(mut runtime) => {
                let status = status_for(&runtime);
                // Label the grid from the demo kit first, so a missing or
                // silent user kit still shows meaningful pad names.
                if let Ok(demo) = drumpad_model::kit::demo::demo_kit() {
                    self.state.apply_kit(&demo);
                }
                // Load the active library kit. On failure the runtime keeps the
                // demo bank it started with, and the dialog explains why.
                let active = self.kits.as_ref().and_then(|service| {
                    let kit = service.active()?.clone();
                    let dir = service.active_dir()?.to_path_buf();
                    Some((kit, dir))
                });
                if let Some((kit, dir)) = active {
                    match runtime.load_kit(&kit, &dir) {
                        Ok(()) => {
                            tracing::info!(kit = %kit.metadata.name, "active kit loaded at startup");
                            self.state.apply_kit(&kit);
                        }
                        Err(error) => {
                            let message = format!("{}: {error}", kit.metadata.name);
                            tracing::warn!(%message, "active kit failed; keeping the demo bank");
                            self.state.report_load_failure(message);
                        }
                    }
                }
                self.state.set_audio_status(status);
                tracing::info!(
                    running = runtime.is_running(),
                    rate = runtime.sample_rate(),
                    format = ?runtime.format(),
                    "MVP audio runtime started"
                );
                self.audio = Some(runtime);
                self.audio_error = None;
            }
            Err(error) => {
                let message = error.to_string();
                tracing::error!(%message, "MVP audio runtime failed to start");
                self.state.set_audio_status(AudioStatus {
                    device: "No audio device".to_owned(),
                    running: false,
                    detail: Some(message.clone()),
                    ..AudioStatus::default()
                });
                self.state.notifications.error(
                    "Audio could not start",
                    format!(
                        "{message}. Drumpad stays open; pads are silent until audio is available."
                    ),
                    DialogAction::labelled(DialogIntent::OpenSettings, "Check audio settings"),
                );
                self.state
                    .notifications
                    .add_dialog_action(DialogAction::labelled(
                        DialogIntent::Dismiss,
                        "Continue silently",
                    ));
                self.audio_error = Some(message);
            }
        }
    }

    /// Keep the egui zoom in sync with the window's logical size.
    ///
    /// The stage layout is designed for 1280x720 points. When Windows scaling
    /// leaves fewer points than that, zoom the whole UI down so the bottom pad
    /// row and status bar stay on screen.
    fn update_fit_zoom(&mut self) {
        let Some(gl_window) = self.gl_window.as_ref() else {
            return;
        };
        let size = gl_window.window.inner_size();
        let scale = gl_window.window.scale_factor().max(0.1);
        let logical = (size.width as f64 / scale, size.height as f64 / scale);
        let zoom = fit_zoom_for(logical);
        if (zoom - self.last_fit_zoom).abs() > f32::EPSILON {
            tracing::info!(
                physical_width = size.width,
                physical_height = size.height,
                scale_factor = scale,
                logical_width = logical.0,
                logical_height = logical.1,
                fit_zoom = zoom,
                canvas_width = logical.0 / f64::from(zoom),
                canvas_height = logical.1 / f64::from(zoom),
                "fit zoom updated"
            );
            self.last_fit_zoom = zoom;
        }
        self.ui.set_fit_zoom(zoom);
    }

    fn poll_audio(&mut self, now: Instant) {
        if now.saturating_duration_since(self.last_audio_poll) < Duration::from_millis(100) {
            return;
        }
        self.last_audio_poll = now;

        let events = {
            let Some(audio) = self.audio.as_mut() else {
                return;
            };
            let _ = audio.tick();
            audio.drain_events()
        };

        for event in events {
            self.handle_audio_event(event);
        }

        let Some(audio) = self.audio.as_ref() else {
            return;
        };
        let mut status = status_for(audio);
        status.voices = audio.active_voices();
        self.state.set_audio_status(status);
    }

    fn handle_audio_event(&mut self, event: AudioSystemEvent) {
        match event {
            AudioSystemEvent::Started(format) => {
                tracing::info!(device = %format.device_name, "audio stream started");
            }
            AudioSystemEvent::Reconnected(format) => {
                self.state
                    .notifications
                    .success(format!("Audio reconnected: {}", format.device_name));
            }
            AudioSystemEvent::DeviceLost(reason) => {
                self.state
                    .notifications
                    .warning(format!("Audio device lost: {reason}. Reconnecting..."));
            }
            AudioSystemEvent::DeviceFallback { used, .. } => {
                self.state
                    .notifications
                    .info(format!("Using fallback audio device: {used}"));
            }
            AudioSystemEvent::Disconnected(reason) => {
                self.state
                    .notifications
                    .warning(format!("Audio disconnected: {reason}"));
            }
            AudioSystemEvent::Failed(reason) => {
                self.state.notifications.error(
                    "Audio stream failed",
                    reason,
                    DialogAction::new(DialogIntent::OpenSettings),
                );
                self.state
                    .notifications
                    .add_dialog_action(DialogAction::labelled(
                        DialogIntent::Dismiss,
                        "Continue silently",
                    ));
            }
        }
    }

    fn dispatch(&mut self, action: Action) {
        let effects = dispatch(&mut self.state, action);
        self.apply(effects);
    }

    /// Refresh the UI kit catalogue from the durable library.
    fn sync_catalogue(&mut self) {
        let Some(service) = self.kits.as_mut() else {
            return;
        };
        if let Err(error) = service.refresh(&mut self.state.kits) {
            tracing::warn!(%error, "kit catalogue refresh failed");
            self.state
                .notifications
                .warning(format!("Kit list could not refresh: {error}"));
        }
    }

    /// Push a kit into the running audio runtime.
    fn apply_kit_to_audio(
        &mut self,
        kit: &drumpad_model::kit::Kit,
        dir: &std::path::Path,
    ) -> Result<(), String> {
        let Some(audio) = self.audio.as_mut() else {
            return Err("audio is not running".to_owned());
        };
        audio
            .load_kit(kit, dir)
            .map_err(|error| format!("{}: {error}", kit.metadata.name))
    }

    fn activate_kit(&mut self, kit: &drumpad_model::kit::Kit, dir: &std::path::Path) {
        match self.apply_kit_to_audio(kit, dir) {
            Ok(()) => {
                tracing::info!(kit = %kit.metadata.name, "kit loaded into the audio runtime");
                self.state.apply_kit(kit);
                self.state.kits.set_dirty(false);
            }
            Err(error) => self.state.report_load_failure(error),
        }
    }

    /// Execute one durable kit request against [`KitService`].
    fn handle_kit_request(&mut self, request: crate::state::kit::KitRequest) {
        use crate::state::kit::KitRequest;

        if self.kits.is_none() {
            self.state.notifications.error(
                "Kit library is unavailable",
                "The app could not open its kit folder.",
                DialogAction::new(DialogIntent::OpenLogFolder),
            );
            return;
        }

        match request {
            KitRequest::Load(ui_id) => {
                let outcome = self.kits.as_mut().expect("checked above").load_ui(ui_id.0);
                match outcome {
                    Ok((kit, dir)) => match self.apply_kit_to_audio(&kit, &dir) {
                        Ok(()) => {
                            self.state.apply_kit(&kit);
                            self.state.kits.finish_load(ui_id, Ok(()));
                            self.state.kits.set_dirty(false);
                        }
                        Err(error) => {
                            self.state.kits.finish_load(ui_id, Err(error.clone()));
                            self.state.report_load_failure(error);
                        }
                    },
                    Err(error) => {
                        self.state.kits.finish_load(ui_id, Err(error.clone()));
                        self.state.report_load_failure(error);
                    }
                }
                self.sync_catalogue();
            }
            KitRequest::Create(ui_id) => {
                let name = self
                    .state
                    .kits
                    .get(ui_id)
                    .map(|kit| kit.name.clone())
                    .unwrap_or_default();
                let outcome = self.kits.as_mut().expect("checked above").create(&name);
                match outcome {
                    Ok((kit, dir)) => {
                        self.sync_catalogue();
                        self.activate_kit(&kit, &dir);
                    }
                    Err(error) => {
                        self.state.notifications.error(
                            "Kit could not be created",
                            error,
                            DialogAction::new(DialogIntent::Dismiss),
                        );
                    }
                }
            }
            KitRequest::Save(ui_id) => {
                let outcome = self.kits.as_mut().expect("checked above").save_active();
                match outcome {
                    Ok(()) => {
                        self.state.kits.set_dirty(false);
                        self.state.kits.finish_load(ui_id, Ok(()));
                        self.state.notifications.success("Kit saved.");
                        self.sync_catalogue();
                    }
                    Err(error) => {
                        self.state.notifications.error(
                            "Kit could not be saved",
                            error,
                            DialogAction::new(DialogIntent::Dismiss),
                        );
                    }
                }
            }
            KitRequest::Delete(ui_id) => {
                let outcome = self
                    .kits
                    .as_mut()
                    .expect("checked above")
                    .delete_ui(ui_id.0);
                match outcome {
                    Ok(()) => {
                        self.sync_catalogue();
                        self.state.notifications.success("Kit deleted.");
                        let fallback = self.kits.as_ref().and_then(|service| {
                            let kit = service.active()?.clone();
                            let dir = service.active_dir()?.to_path_buf();
                            Some((kit, dir))
                        });
                        if let Some((kit, dir)) = fallback {
                            self.activate_kit(&kit, &dir);
                        }
                    }
                    Err(error) => {
                        self.state.notifications.error(
                            "Kit could not be deleted",
                            error,
                            DialogAction::new(DialogIntent::Dismiss),
                        );
                    }
                }
            }
            KitRequest::Rename(ui_id) => {
                let name = self
                    .state
                    .kits
                    .get(ui_id)
                    .map(|kit| kit.name.clone())
                    .unwrap_or_default();
                let outcome = self
                    .kits
                    .as_mut()
                    .expect("checked above")
                    .rename_ui(ui_id.0, &name);
                match outcome {
                    Ok(()) => {
                        self.sync_catalogue();
                        self.state.notifications.success("Kit renamed.");
                    }
                    Err(error) => {
                        self.state.notifications.error(
                            "Kit could not be renamed",
                            error,
                            DialogAction::new(DialogIntent::Dismiss),
                        );
                    }
                }
            }
        }
    }
    fn apply(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::None => {}
                Effect::Trigger { pad, velocity } => {
                    if let Some(audio) = self.audio.as_ref() {
                        audio.trigger(pad.index() as u8, velocity);
                    } else {
                        tracing::debug!("trigger dropped: no audio runtime");
                    }
                    tracing::trace!(pad = pad.number(), velocity, "trigger");
                }
                Effect::Release { pad } => {
                    if let Some(audio) = self.audio.as_ref() {
                        audio.release(pad.index() as u8);
                    }
                    tracing::trace!(pad = pad.number(), "release");
                }
                Effect::Panic => {
                    if let Some(audio) = self.audio.as_ref() {
                        audio.panic();
                    }
                    tracing::debug!("panic: stop all voices");
                }
                Effect::Metronome(on) => tracing::debug!(on, "metronome"),
                Effect::Kit(request) => {
                    if self.kits.is_some() {
                        self.handle_kit_request(request);
                    } else {
                        tracing::info!(?request, "kit request before the library is enabled");
                        self.state.notifications.info(
                            "The kit library is not enabled in this build; the demo kit stays loaded.",
                        );
                    }
                }
                Effect::DuplicateKit(source) => {
                    let outcome = self
                        .kits
                        .as_mut()
                        .map(|service| service.duplicate_ui(source.0))
                        .unwrap_or_else(|| Err("kit library is unavailable".to_owned()));
                    match outcome {
                        Ok(()) => {
                            self.sync_catalogue();
                            self.state.notifications.success("Kit duplicated.");
                        }
                        Err(error) => {
                            self.state.notifications.error(
                                "Kit could not be duplicated",
                                error,
                                DialogAction::new(DialogIntent::Dismiss),
                            );
                        }
                    }
                }
                Effect::ImportKit { name, folder } => {
                    let outcome = self
                        .kits
                        .as_mut()
                        .map(|service| service.import_folder(&name, &folder))
                        .unwrap_or_else(|| Err("kit library is unavailable".to_owned()));
                    match outcome {
                        Ok(outcome) => {
                            tracing::info!(
                                kit = %outcome.kit.metadata.name,
                                imported = outcome.imported,
                                rejected = outcome.rejected.len(),
                                skipped = outcome.skipped,
                                "kit imported from folder"
                            );
                            self.sync_catalogue();
                            match self.apply_kit_to_audio(&outcome.kit, &outcome.kit_dir) {
                                Ok(()) => self.state.apply_kit(&outcome.kit),
                                Err(error) => self.state.report_load_failure(error),
                            }
                            self.state.notifications.success(format!(
                                "{}: {}",
                                outcome.kit.metadata.name, outcome.summary
                            ));
                            if !outcome.rejected.is_empty() {
                                self.state.notifications.warning(format!(
                                    "{} file(s) were skipped: {}",
                                    outcome.rejected.len(),
                                    outcome.rejected.join("; ")
                                ));
                            }
                        }
                        Err(error) => {
                            tracing::warn!(%error, "kit import failed");
                            self.state.notifications.error(
                                "Kit import failed",
                                error,
                                DialogAction::new(DialogIntent::OpenSettings),
                            );
                            self.state
                                .notifications
                                .add_dialog_action(DialogAction::labelled(
                                    DialogIntent::Dismiss,
                                    "Choose another folder",
                                ));
                        }
                    }
                }
                Effect::PersistPreferences(preferences) => {
                    let path = self.options.preferences_path();
                    match preferences.save(&path) {
                        Ok(()) => {
                            self.options.preferences = *preferences;
                            self.ui.sync_theme(&self.options.preferences);
                        }
                        Err(error) => {
                            tracing::error!(%error, "could not save preferences");
                            self.state.notifications.error(
                                "Settings could not be saved",
                                error.to_string(),
                                DialogAction::new(DialogIntent::OpenLogFolder),
                            );
                        }
                    }
                }
                Effect::PreferencesChanged => {
                    self.ui.sync_theme(self.state.settings.saved());
                }
                Effect::OpenLogFolder => {
                    open_folder(self.options.paths.log_dir());
                }
                Effect::OpenCrashFolder => {
                    open_folder(self.options.paths.crash_dir());
                }
                Effect::RelinkSamples => {
                    self.state
                        .notifications
                        .info("Relink flow requested. The sample service owns the file search.");
                }
                Effect::PadSettingsChanged(pad) => {
                    tracing::debug!(pad = pad.number(), "pad settings changed");
                    self.state.kits.set_dirty(true);
                }
                Effect::WaveformChanged => {
                    tracing::debug!("waveform changed");
                    self.state.kits.set_dirty(true);
                }
            }
        }
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop, window_id: WindowId) {
        self.update_fit_zoom();
        let Some(gl_window) = self.gl_window.as_ref() else {
            return;
        };
        if gl_window.window.id() != window_id {
            return;
        }

        let now = Instant::now();
        let delta = now.saturating_duration_since(self.last_frame);
        let delta = delta.min(MAX_FRAME_DELTA);
        self.last_frame = now;

        if !self.pending_drop.is_empty() {
            let paths = std::mem::take(&mut self.pending_drop);
            self.dispatch(Action::BeginDrop {
                paths,
                target: None,
            });
        }

        let mut actions = Vec::new();
        {
            let Self {
                egui,
                state,
                ui,
                gl_window,
                ..
            } = self;
            let Some(egui) = egui.as_mut() else {
                return;
            };
            let Some(gl_window) = gl_window.as_ref() else {
                return;
            };
            egui.run(&gl_window.window, |ctx| {
                actions = ui.show(ctx, state);
            });
        }

        let queued = std::mem::take(&mut self.queued_actions);
        for action in queued.into_iter().chain(actions) {
            self.dispatch(action);
        }

        self.state.tick(delta);
        self.poll_audio(now);

        {
            let Self {
                egui,
                gl_window,
                gl,
                ..
            } = self;
            let (Some(egui), Some(gl_window), Some(gl)) =
                (egui.as_mut(), gl_window.as_ref(), gl.as_ref())
            else {
                return;
            };
            unsafe {
                use glow::HasContext as _;
                // Clear before painting so a resize never shows stale pixels.
                gl.clear_color(0.043, 0.051, 0.063, 1.0);
                gl.clear(glow::COLOR_BUFFER_BIT);
            }
            egui.paint(&gl_window.window);
            if let Err(error) = gl_window.swap_buffers() {
                tracing::error!(%error, "could not swap buffers");
                event_loop.exit();
                return;
            }
        }

        // 60 fps while idle; animations and repaint requests can lower the delay.
        let delay = if self.state.is_animating() || self.repaint_delay.is_zero() {
            FRAME_INTERVAL.min(Duration::from_millis(1))
        } else {
            self.repaint_delay.min(FRAME_INTERVAL)
        };
        if let Some(deadline) = Instant::now().checked_add(delay) {
            event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
        }
        if let Some(gl_window) = self.gl_window.as_ref() {
            gl_window.window.request_redraw();
        }
    }

    fn keyboard(&mut self, event: &winit::event::KeyEvent) {
        let PhysicalKey::Code(key) = event.physical_key else {
            return;
        };

        if event.state == ElementState::Pressed {
            // 1. Global shortcuts win, and never reach a pad.
            if let Some(action) = shortcut(key, self.modifiers, true) {
                self.dispatch(action);
                return;
            }

            // 2. Inspector key capture.
            if ui::capture_key(&mut self.state, self.ui.local_mut(), key) {
                return;
            }

            // 3. The pad router, unless a text field wants the keyboard.
            let wants_keyboard = self
                .egui
                .as_ref()
                .is_some_and(|egui| egui.egui_ctx.wants_keyboard_input());
            let transport = key == winit::keyboard::KeyCode::F1
                || key == winit::keyboard::KeyCode::F2
                || key == winit::keyboard::KeyCode::F3
                || key == winit::keyboard::KeyCode::F4
                || key == winit::keyboard::KeyCode::F5;
            if !routes_to_pads(self.modifiers) || (wants_keyboard && !transport) {
                return;
            }
            if let Some(action) = self.state.handle_key(KeyInput::press(key)) {
                self.pad_action(action);
            }
        } else if self.state.router.is_down(key)
            && let Some(action) = self.state.handle_key(KeyInput::release(key))
        {
            self.pad_action(action);
        }
    }

    fn pad_action(&mut self, action: crate::state::keymap::KeyAction) {
        use crate::state::keymap::KeyAction;
        match action {
            KeyAction::Pad(pad) => {
                self.dispatch(Action::TriggerPad { pad, velocity: 100 });
            }
            KeyAction::PadRelease(pad) => {
                self.dispatch(Action::ReleasePad(pad));
            }
            KeyAction::Transport(action) => self.dispatch(Action::Transport(action)),
            KeyAction::Unmapped(_) => {}
        }
    }
}

impl ApplicationHandler<UserEvent> for AppShell {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gl_window.is_none() {
            self.create_display(event_loop);
            if self.audio_error.is_none() && !self.audio_attempted {
                self.start_audio();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let same_window = self
            .gl_window
            .as_ref()
            .is_some_and(|gl_window| gl_window.window.id() == window_id);
        if !same_window {
            return;
        }

        match &event {
            WindowEvent::CloseRequested | WindowEvent::Destroyed => {
                event_loop.exit();
                return;
            }
            WindowEvent::RedrawRequested => {
                self.redraw(event_loop, window_id);
                return;
            }
            WindowEvent::Resized(size) => {
                if let Some(gl_window) = self.gl_window.as_ref() {
                    gl_window.resize(*size);
                    gl_window.window.request_redraw();
                }
                self.update_fit_zoom();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                // egui-winit updates pixels_per_point on the same event; the
                // fit zoom is recomputed from the new logical size next frame.
                tracing::info!(scale_factor, "DPI scale changed");
                if let Some(gl_window) = self.gl_window.as_ref() {
                    gl_window.window.request_redraw();
                }
                self.update_fit_zoom();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                self.keyboard(event);
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }
            WindowEvent::DroppedFile(path) => {
                self.pending_drop.push(path.clone());
                if let Some(gl_window) = self.gl_window.as_ref() {
                    gl_window.window.request_redraw();
                }
            }
            WindowEvent::Focused(false) => {
                self.state.router.clear();
                self.apply(vec![Effect::Panic]);
                if let Some(gl_window) = self.gl_window.as_ref() {
                    gl_window.window.request_redraw();
                }
            }
            _ => {}
        }

        if let (Some(egui), Some(gl_window)) = (self.egui.as_mut(), self.gl_window.as_ref()) {
            let response = egui.on_window_event(&gl_window.window, &event);
            if response.repaint {
                gl_window.window.request_redraw();
            }
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Redraw(delay) => self.repaint_delay = delay,
        }
    }

    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: winit::event::StartCause) {
        if let winit::event::StartCause::ResumeTimeReached { .. } = cause
            && let Some(gl_window) = self.gl_window.as_ref()
        {
            gl_window.window.request_redraw();
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(mut audio) = self.audio.take() {
            audio.stop();
        }
        if let Some(egui) = self.egui.as_mut() {
            egui.destroy();
        }
        tracing::info!("window closed; shutting down");
    }
}

fn status_for(runtime: &MvpAudioRuntime) -> AudioStatus {
    let mut status = AudioStatus {
        running: runtime.is_running(),
        sample_rate: runtime.sample_rate(),
        ..AudioStatus::default()
    };
    if let Some(format) = runtime.format() {
        status.device = format.device_name.clone();
        status.sample_rate = format.sample_rate;
        status.buffer_frames = format.buffer_frames;
        status.detail = format.fallback_reason.clone();
    } else if let drumpad_audio::AudioSystemState::Failed { reason } = runtime.state() {
        status.detail = Some(reason.clone());
    }
    status
}

fn open_folder(path: &std::path::Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer.exe").arg(path).spawn();
    }
    #[cfg(not(windows))]
    {
        tracing::info!(path = %path.display(), "open folder requested");
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    /// QA's exact client size before the fix: 1382x777 logical at 125%.
    #[test]
    fn a_1382x777_client_scales_the_1440x900_design_down() {
        let zoom = fit_zoom_for((1382.4, 777.6));
        assert!(
            zoom < 1.0,
            "the design does not fit, zoom must shrink: {zoom}"
        );
        let canvas_w = 1382.4 / f64::from(zoom);
        let canvas_h = 777.6 / f64::from(zoom);
        assert!(canvas_w >= DESIGN_WIDTH - 1.0, "canvas width {canvas_w}");
        assert!(canvas_h >= DESIGN_HEIGHT - 1.0, "canvas height {canvas_h}");
    }

    /// QA's 125% case: a 1920x1080 physical monitor with a 1536x816 logical
    /// work area (taskbar removed).
    #[test]
    fn a_125_percent_small_work_area_fits_the_stage_layout() {
        let work = (1536.0, 816.0);
        let (inner, min) = fitted_window_sizes(work);
        assert!(inner.width <= work.0 * WORK_WIDTH_FRACTION + 0.001);
        assert!(inner.height <= work.1 * WORK_HEIGHT_FRACTION + 0.001);
        assert!(min.width <= inner.width && min.height <= inner.height);

        // Leave room for the title bar and the OS default window position.
        let chrome_height = 32.0;
        let default_top = work.1 * 0.09;
        assert!(
            default_top + chrome_height + inner.height <= work.1,
            "window bottom {} exceeds the work area {}",
            default_top + chrome_height + inner.height,
            work.1
        );

        let zoom = fit_zoom_for((inner.width, inner.height));
        assert!(zoom < 1.0, "the UI must scale down on this work area");
        let canvas_w = inner.width / f64::from(zoom);
        let canvas_h = inner.height / f64::from(zoom);
        assert!(canvas_w >= DESIGN_WIDTH - 1.0, "canvas width {canvas_w}");
        assert!(canvas_h >= DESIGN_HEIGHT - 1.0, "canvas height {canvas_h}");
    }

    #[test]
    fn a_large_monitor_keeps_the_design_size_and_100_percent_zoom() {
        let (inner, min) = fitted_window_sizes((1920.0, 1080.0));
        assert_eq!((inner.width, inner.height), DESIRED_WINDOW);
        assert_eq!(fit_zoom_for((inner.width, inner.height)), 1.0);
        assert!(min.width <= inner.width && min.height <= inner.height);
    }

    #[test]
    fn anything_shorter_than_the_design_height_zooms_out() {
        assert!(fit_zoom_for((1920.0, 899.0)) < 1.0);
        assert!(fit_zoom_for((1439.0, 900.0)) < 1.0);
        assert_eq!(fit_zoom_for((1440.0, 900.0)), 1.0);
    }

    #[test]
    fn a_narrow_client_scales_down_to_fit_the_design() {
        let zoom = fit_zoom_for((1100.0, 620.0));
        assert!(zoom < 1.0);
        assert!(1100.0 / f64::from(zoom) >= DESIGN_WIDTH - 1.0);
        assert!(620.0 / f64::from(zoom) >= DESIGN_HEIGHT - 1.0);
    }

    #[test]
    fn fit_zoom_never_enlarges_and_is_bounded() {
        assert_eq!(fit_zoom_for((5000.0, 4000.0)), 1.0);
        assert!(fit_zoom_for((640.0, 360.0)) >= 0.5);
        assert_eq!(fit_zoom_for((0.0, 0.0)), 1.0);
    }
}
