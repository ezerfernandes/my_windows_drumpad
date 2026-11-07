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

use crate::app::actions::{dispatch, Action};
use crate::app::hotkeys::{routes_to_pads, shortcut};
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
    audio_attempted: bool,
    audio_error: Option<String>,
    last_audio_poll: Instant,
}

impl AppShell {
    fn new(options: AppOptions, proxy: EventLoopProxy<UserEvent>) -> Self {
        let state = AppState::new(options.preferences.clone());
        let ui = UiRoot::new(&options.preferences);
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
            audio_attempted: false,
            audio_error: None,
            last_audio_poll: Instant::now(),
        }
    }

    fn create_display(&mut self, event_loop: &ActiveEventLoop) {
        let attributes = winit::window::WindowAttributes::default()
            .with_title(self.options.title.clone())
            .with_inner_size(winit::dpi::LogicalSize {
                width: 1440.0,
                height: 900.0,
            })
            .with_min_inner_size(winit::dpi::LogicalSize {
                width: 960.0,
                height: 600.0,
            })
            .with_visible(false);

        let template = glutin::config::ConfigTemplateBuilder::new()
            .with_depth_size(0)
            .with_stencil_size(0)
            .with_transparency(false);

        let (window, gl_config) = match {
            glutin_winit::DisplayBuilder::new()
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
                })
        } {
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
            .map(|window| window.window_handle().map(|handle| handle.as_raw()).ok())
            .flatten();

        let context_attributes =
            glutin::context::ContextAttributesBuilder::new().build(raw_handle);
        let fallback_attributes = glutin::context::ContextAttributesBuilder::new()
            .with_context_api(glutin::context::ContextApi::Gles(None))
            .build(raw_handle);
        let not_current = unsafe {
            gl_display
                .create_context(&gl_config, &context_attributes)
                .or_else(|_| {
                    gl_display.create_context(&gl_config, &fallback_attributes)
                })
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
                    window
                        .window_handle()
                        .expect("window handle")
                        .as_raw(),
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
        let gl = unsafe {
            glow::Context::from_loader_function(|name| {
                let name = std::ffi::CString::new(name).expect("valid GL name");
                gl_window.get_proc_address(&name)
            })
        };
        let gl = Arc::new(gl);

        let egui = egui_glow::EguiGlow::new(event_loop, gl.clone(), None, None, true);
        let proxy = std::sync::Mutex::new(self.proxy.clone());
        egui.egui_ctx
            .set_request_repaint_callback(move |info| {
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
            Ok(runtime) => {
                let status = status_for(&runtime);
                if let Ok(kit) = drumpad_model::kit::demo::demo_kit() {
                    self.state.apply_kit(&kit);
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
                self.state.notifications.add_dialog_action(DialogAction::labelled(
                    DialogIntent::Dismiss,
                    "Continue silently",
                ));
                self.audio_error = Some(message);
            }
        }
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
                self.state.notifications.add_dialog_action(DialogAction::labelled(
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
                    // The kit service belongs to the model pair. Until it is
                    // wired, report the request instead of pretending it ran.
                    tracing::info!(?request, "kit request");
                    self.state.notifications.info(format!(
                        "Kit request queued: {request:?}. The kit service is not wired yet."
                    ));
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
                    self.state.notifications.info(
                        "Relink flow requested. The sample service owns the file search.",
                    );
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
            let (Some(egui), Some(gl_window), Some(gl)) = (egui.as_mut(), gl_window.as_ref(), gl.as_ref())
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
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                // egui-winit updates pixels_per_point on the same event; the
                // layout is rebuilt from the new available rectangle next frame.
                tracing::info!(scale_factor, "DPI scale changed");
                if let Some(gl_window) = self.gl_window.as_ref() {
                    gl_window.window.request_redraw();
                }
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
