//! Uzumaki: a Rust, GPU-rendered frontend for Apple Music on Linux.
//!
//! The UI is Rune. Playback and Apple Music API calls go through a hidden castLabs Electron
//! process (`bridge/`) that drives music.apple.com's own MusicKit player.
//!
//! `cargo run -p uzumaki` opens the app. Headless checks (no Electron, fake data):
//! `cargo run -p uzumaki -- --snap /tmp/x.png --scene album --size 1360x860 --scale 1`
//! (`--list-scenes` lists them).

// Windows: a windowed program (no console window); `win::attach_output` gives its text a place.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod airplay;
mod appearance;
mod artist;
#[cfg(any(target_os = "linux", test))]
mod app_icon;
#[cfg(target_os = "macos")]
mod bundle_icon;
mod art;
mod clock;
mod backdrop;
mod bridge;
mod cast;
mod cache;
mod ctl;
mod desktop;
mod dock_icon;
mod native_theme;
mod framelog;
mod discord;
mod hooks;
mod local;
mod legacy;
#[cfg(target_os = "macos")]
mod logfile;
mod login;
#[cfg(target_os = "macos")]
mod menu_bar_mac;
mod lyrics;
mod media;
mod mixed;
mod models;
mod picture;
mod motion;
// On macOS only the single-instance check is used; the D-Bus service is not. Windows has no
// session bus at all: its media keys are `smtc.rs`.
#[cfg(unix)]
#[cfg_attr(target_os = "macos", allow(dead_code))]
mod mpris;
mod notify;
#[cfg(target_os = "macos")]
mod nowplaying;
#[cfg(target_os = "macos")]
mod raster_mac;
mod scenes;
mod settings;
#[cfg(windows)]
mod smtc;
mod share;
#[cfg(target_os = "macos")]
mod share_mac;
mod symbols;
mod state;
mod theme;
mod tray;
mod ui;
mod resume;
mod update;
#[cfg(windows)]
mod win;

/// Wayland app_id, desktop entry and icon name.
pub const APP_ID: &str = "io.github.hanami117.uzumaki";

/// The app's version: the workspace's (`Cargo.toml`), the one source for About, Settings,
/// `--version`, the update check and the macOS bundle (`packaging/macos/build.sh`).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The build number (the `BUILD` file; see `uzumaki_ctl::BUILD`). It goes up when a package is
/// rebuilt without code changes; the update check compares (version, build).
pub const BUILD: u32 = uzumaki_ctl::BUILD;
/// Where Settings → Support sends you.
pub const SUPPORT_URL: &str = "https://ko-fi.com/hanami117";
/// The copyright line (Settings → About; the macOS About panel and Info.plist spell it out too).
pub const COPYRIGHT: &str = "© 2026 Thomas Lonigro";

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use rune::{App, AppSpec, ImageCache, Tx, Waker};

use crate::bridge::{Bridge, Engine, Event};
use crate::local::LocalPlayer;
use crate::state::{Msg, State};

fn setup(s: &mut State, wake: Waker) {
    symbols::warm();
    s.art.attach(ImageCache::new(wake));
    // The Share submenu's system services, read once a little after start (not in the first frames).
    #[cfg(target_os = "macos")]
    if !rune::is_headless() {
        share_mac::warm();
    }
}

/// Closing the window quits, unless close to tray is on (then it only hides; the tray's Quit,
/// Ctrl+Q and MPRIS Quit still quit). On macOS Rune also runs this when the OS ends the app
/// (Cmd-Q, the app menu's or the Dock's Quit), but then the loop is already exiting, so doing
/// nothing here still quits.
fn on_close(s: &mut State, tx: &mut Tx) {
    if !s.hide_on_close() {
        tx.quit();
    }
}

/// The window gained or lost focus, also while hidden or minimised (for the song notifications).
fn on_focus(s: &mut State, focused: bool, _tx: &mut Tx) {
    s.set_window_focused(focused);
}

/// macOS: Dock or Finder reopen.
#[cfg(target_os = "macos")]
fn on_reopen(s: &mut State, has_visible_windows: bool, _tx: &mut Tx) -> bool {
    dock_icon::reapply();
    s.reopen(has_visible_windows)
}

/// macOS: paints the native window's own background in the app's background colour. AppKit
/// shows it wherever the GPU frame doesn't reach yet: while the window grows (zoom, tiling,
/// full screen, an edge drag) the frame for the new size lands a moment after the window has
/// it. Its default is white in light mode, so the strip flashed white. Linux doesn't need this:
/// a Wayland window takes its new size only with the client's next frame. Runs each time Rune
/// creates the native window, and again (`sync_window_bg`) when the theme changes.
#[cfg(target_os = "macos")]
fn on_window(handle: &rune::WindowHandle) {
    WINDOW.with(|w| *w.borrow_mut() = Some(handle.clone()));
    paint_window_bg(handle);    dock_icon::reapply();
}

#[cfg(target_os = "macos")]
thread_local! {
    /// The native window, kept (on the UI thread) to repaint its background on a theme change.
    static WINDOW: std::cell::RefCell<Option<rune::WindowHandle>> = const { std::cell::RefCell::new(None) };
}

/// Called by `theme::apply` on the UI thread when the palette flipped between dark and light.
#[cfg(target_os = "macos")]
pub(crate) fn sync_window_bg() {
    WINDOW.with(|w| {
        if let Some(h) = w.borrow().as_ref() {
            paint_window_bg(h);
        }
    });
}
#[cfg(windows)]
thread_local! {
    /// Windows: the native window, kept to make its title bar follow the palette (`win::sync_caption`).
    static WINDOW: std::cell::RefCell<Option<rune::WindowHandle>> = const { std::cell::RefCell::new(None) };
}
#[cfg(windows)]
fn on_window(handle: &rune::WindowHandle) {
    WINDOW.with(|w| *w.borrow_mut() = Some(handle.clone()));
    win::on_window(handle);
}
#[cfg(windows)]
pub(crate) fn sync_window_bg() {
    WINDOW.with(|w| {
        if let Some(h) = w.borrow().as_ref() {
            win::sync_caption(h);
        }
    });
}
#[cfg(not(any(target_os = "macos", windows)))]
pub(crate) fn sync_window_bg() {}

/// The window's native content view (UI thread only): where Share's services anchor their sheets.
#[cfg(target_os = "macos")]
#[cfg_attr(test, allow(dead_code))]
pub(crate) fn content_view() -> Option<objc2::rc::Retained<objc2_app_kit::NSView>> {
    use rune::raw_window_handle::RawWindowHandle;
    WINDOW.with(|w| {
        let w = w.borrow();
        w.as_ref()?
            .with_raw_handle(|h| {
                let RawWindowHandle::AppKit(h) = h.as_raw() else { return None };
                // SAFETY: an NSView, alive for the callback; retaining it keeps it past that.
                unsafe { objc2::rc::Retained::retain(h.ns_view.as_ptr().cast::<objc2_app_kit::NSView>()) }
            })
            .ok()
            .flatten()
    })
}

#[cfg(target_os = "macos")]
fn paint_window_bg(handle: &rune::WindowHandle) {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use rune::raw_window_handle::RawWindowHandle;

    // Rune colours are linear; NSColor takes sRGB-encoded components (Rune tags its layer sRGB).
    let bg = theme::PAL.bg;
    let [r, g, b] = [bg.r, bg.g, bg.b].map(|c| f64::from(rune::color::srgb_encode(c)));
    let _ = handle.with_raw_handle(|h| {
        let RawWindowHandle::AppKit(h) = h.as_raw() else { return };
        // SAFETY: on the UI (main) thread, with the view alive for the callback.
        unsafe {
            let view = h.ns_view.as_ptr().cast::<AnyObject>();
            let window: Option<Retained<AnyObject>> = msg_send![view, window];
            let Some(window) = window else { return };
            let color: Retained<AnyObject> = msg_send![class!(NSColor), colorWithSRGBRed: r, green: g, blue: b, alpha: 1.0f64];
            let _: () = msg_send![&*window, setBackgroundColor: &*color];
        }
    });
}

/// The window's opening size in points. Windows: made smaller where the primary screen's work
/// area (the screen less the taskbar) is smaller, so the window never opens larger than the
/// screen of a small laptop. Headless runs take `--size`.
fn window_size() -> (f32, f32) {
    const WANTED: (f32, f32) = (1360.0, 860.0);
    #[cfg(windows)]
    if !rune::is_headless() {
        return win::window_size(WANTED, (640.0, 500.0));
    }
    WANTED
}

fn main() {
    #[cfg(windows)]
    win::attach_output();
    if std::env::args().skip(1).any(|a| a == "--version" || a == "-V") {
        println!("uzumaki {}", uzumaki_ctl::version_line());
        return;
    }
    // Wayland app_id / X11 WM_CLASS: the dock, taskbar and window switcher find the icon
    // through the desktop entry of the same name (assets/io.github.hanami117.uzumaki.desktop).
    rune::set_app_id(APP_ID);
    let spec = AppSpec::<State> {
        scenes: scenes::SCENES,
        setup: Some(setup),
        on_key: Some(ui::on_key),
        poll: Some(ui::poll),
        min_size: Some((640.0, 500.0)),
        ..AppSpec::new("Uzumaki", window_size(), State::offline, ui::view)
    };
    // macOS: the traffic lights pinned over the sidebar rail (`theme::TRAFFIC_LIGHTS`).
    // Tab visits buttons too, not only fields and lists (macOS's default without "Keyboard
    // navigation"): Tab, then Enter or the arrows, is how the app works without a mouse.
    let app = App::new(spec)
        .chrome(rune::Chrome { traffic_lights: Some(theme::TRAFFIC_LIGHTS), ..rune::Chrome::DEFAULT })
        .tab_visits_controls(true);
    // macOS: Rune's standard Edit (Undo … Select All, keys to text fields first) and Window
    // (Minimize, Close) menus; the app's own menus are `ui/menu_bar.rs`.
    #[cfg(target_os = "macos")]
    let app = app.standard_menus();

    if rune::is_headless() {
        app.run();
        return;
    }
    // macOS: opened from the Finder or the Dock, stderr is /dev/null; keep the messages in
    // ~/Library/Logs/Uzumaki instead.
    #[cfg(target_os = "macos")]
    logfile::redirect_stderr();
    // `--hidden`: start in the background with no window (for `uzu start`), like a daemon.
    let hidden = std::env::args().any(|a| a == "--hidden");
    // A second copy for tests, next to the user's: no single-instance check and no MPRIS (pair it
    // with a scratch UZUMAKI_PROFILE_DIR, XDG_CONFIG_HOME, XDG_CACHE_HOME, TMPDIR and
    // UZUMAKI_CTL_SOCKET).
    let test_instance = std::env::var_os("UZUMAKI_TEST_INSTANCE").is_some_and(|v| !v.is_empty());
    #[cfg(unix)]
    let running = if test_instance {
        None
    } else if hidden {
        mpris::running()
    } else {
        mpris::raise_running()
    };
    #[cfg(not(unix))]
    let running: Option<media::Running> = None;
    // Windows: this copy may bring the running one's window to the front, and the taskbar, the
    // media overlay and the notifications know the app by its id.
    #[cfg(windows)]
    {
        win::allow_focus();
        win::set_app_id();
    }
    // macOS and Windows have no session bus for MPRIS to find another copy: ask the control
    // socket (on Windows a named pipe) instead.
    let running = running.or_else(|| {
        (cfg!(any(target_os = "macos", windows)) && !test_instance && ctl::other_copy_running(!hidden)).then_some(media::Running::Current)
    });
    match running {
        Some(media::Running::Current) => {
            eprintln!("uzumaki: already running{}", if hidden { "" } else { "; showed its window" });
            return;
        }
        Some(media::Running::Legacy) => {
            eprintln!("uzumaki: the old Cadence is still running. Quit it (Ctrl+Q) and start Uzumaki again.");
            return;
        }
        None => legacy::migrate(), // first start after the rename: move Cadence's data over
    }
    // macOS: the chosen theme reaches the native chrome (traffic lights, dialogs, tooltips, menus)
    // and the window's own background before the window exists, so a forced Light or Dark never
    // flashes the computer's appearance first. `ui::view` keeps both in step afterwards.
    #[cfg(target_os = "macos")]
    {
        let theme = settings::Settings::load().theme;
        native_theme::sync(theme);
        theme::apply(theme.is_light(native_theme::os_dark(theme, true)));
    }
    // macOS start at login in the background: a login item gets no `--hidden`, so with both
    // switches on (Settings → General) the app starts windowless and `login::watch_launch` opens
    // the window at once unless this launch is the login one. Linux writes `--hidden` into the
    // autostart entry instead.
    #[cfg(target_os = "macos")]
    let login_check = !hidden && !test_instance && settings::Settings::load().start_hidden && login::read().on;
    #[cfg(not(target_os = "macos"))]
    let login_check = false;
    let hidden = hidden || login_check;
    let app = if hidden { app.windowless() } else { app };
    let app = app.keep_running().on_close(on_close).on_focus(on_focus);
    // Animated artwork pauses while the window can't draw (minimised, covered).
    let frames_handle = app.handle();
    let app = app.on_frame_encoded(move |f| state::motion::frame_outcome(&frames_handle, f.outcome()));
    #[cfg(target_os = "macos")]
    let app = app.on_reopen(on_reopen).on_window(on_window);
    #[cfg(windows)]
    let app = app.on_window(on_window);
    // Windows: the name and icon behind the app id, for notifications and the media overlay of a
    // copy that was only unpacked (the installer's shortcut carries the same id).
    #[cfg(windows)]
    if !test_instance {
        std::thread::spawn(win::register_app_id);
    }

    // Bridge events and replies arrive on its reader thread. MPRIS hears playback events there
    // too (so media keys and panel widgets stay current while the window isn't drawing), then
    // the message goes to the UI thread.
    let handle = app.handle();
    // Show the library from disk while Electron starts.
    {
        let handle = handle.clone();
        std::thread::spawn(move || {
            let cached = cache::Cache::default_location().load_all();
            handle.update(move |s: &mut State, tx| {
                s.landing(tx);
                s.apply_cached(cached);
            });
        });
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    let mpris_slot: Arc<OnceLock<mpris::Mpris>> = Arc::new(OnceLock::new());
    #[cfg(target_os = "macos")]
    let mpris_slot: Arc<OnceLock<nowplaying::NowPlaying>> = Arc::new(OnceLock::new());
    #[cfg(windows)]
    let mpris_slot: Arc<OnceLock<smtc::Smtc>> = Arc::new(OnceLock::new());
    // True while the local-files player is in charge: MPRIS then follows its playback events
    // and ignores the (paused) bridge's.
    let local_active = Arc::new(AtomicBool::new(false));
    let sink = {
        let handle = handle.clone();
        let mpris_slot = mpris_slot.clone();
        let local_active = local_active.clone();
        move |msg: Msg| {
            if let Some(m) = mpris_slot.get() {
                let local = local_active.load(Ordering::SeqCst);
                let playback = |e: &Event| matches!(e, Event::NowPlaying(_) | Event::PlayState(_) | Event::Time { .. } | Event::Stopped);
                match &msg {
                    Msg::Bridge(e) if !(local && playback(e)) => m.observe(e),
                    Msg::Local { event, .. } if local => m.observe(event),
                    _ => {}
                }
            }
            handle.update_if(move |s: &mut State, tx| s.apply(msg, tx));
        }
    };
    // AirPlay: created idle (no thread, no network) before the players, which ask it where the
    // audio goes; it starts working when the picker opens or a speaker is chosen.
    {
        let sink = sink.clone();
        airplay::init(move |e| sink(Msg::AirPlay(e)));
    }
    // Chromecast: created idle; used only while Settings → Speakers → Chromecast is on.
    {
        let sink = sink.clone();
        cast::init(move |e| sink(Msg::Cast(e)));
    }
    let local = {
        let sink = sink.clone();
        LocalPlayer::spawn(move |generation, event| sink(Msg::Local { generation, event }), local_active)
    };
    // `--login` shows the sign-in window, unless the last run was already signed in (then it would
    // only flash up until the page reports it; Settings › Sign in still opens it).
    let debug_login = std::env::args().any(|a| a == "--login");
    let show_login = debug_login && !cache::Cache::default_location().signed_in();
    // The bridge starts when Apple Music is wanted: at launch (below, in `init`), after the
    // onboarding's choice, or on a switch in Settings → Account. Local only never starts it.
    // Every bridge reports through the same sink, so the media keys follow a late one too.
    let engine = {
        let sink = sink.clone();
        let log_path = bridge::log_path();
        Engine::<Msg>::new(log_path.clone(), move |show| Bridge::spawn(show, &log_path, sink.clone()))
    };
    {
        // Media keys go through the app state, which knows which player is in charge.
        let h = handle.clone();
        let on_command = move |cmd: media::Command| match cmd {
            media::Command::Play => h.update(|s: &mut State, _| s.play()),
            media::Command::Pause => h.update(|s: &mut State, _| s.pause()),
            media::Command::Next => h.update(|s: &mut State, _| s.next()),
            media::Command::Previous => h.update(|s: &mut State, _| s.previous()),
            media::Command::SeekTo(t) => h.update(move |s: &mut State, _| s.seek_seconds(t)),
            media::Command::Volume(v) => h.update(move |s: &mut State, _| s.set_volume(v)),
            media::Command::Raise => tray::show_window(&h),
            media::Command::Quit => h.quit(),
        };
        // macOS has no D-Bus: media keys and Control Center go through MediaPlayer.framework.
        // Windows: the System Media Transport Controls (`smtc.rs`).
        #[cfg(all(unix, not(target_os = "macos")))]
        let started = if test_instance { Err("off in test instances".to_string()) } else { mpris::Mpris::start(on_command) };
        #[cfg(target_os = "macos")]
        let started = if test_instance { Err("off in test instances".to_string()) } else { nowplaying::NowPlaying::start(on_command) };
        #[cfg(windows)]
        let started = if test_instance { Err("off in test instances".to_string()) } else { smtc::Smtc::start(on_command) };
        match started {
            Ok(m) => {
                let _ = mpris_slot.set(m);
            }
            #[cfg(all(unix, not(target_os = "macos")))]
            Err(e) => eprintln!("uzumaki: MPRIS unavailable (media keys won't work): {e}"),
            #[cfg(any(target_os = "macos", windows))]
            Err(e) => eprintln!("uzumaki: media keys unavailable: {e}"),
        }
    }

    // `uzu` talks to this socket; it goes away with `_ctl` when the app ends.
    let _ctl = ctl::start(handle.clone());
    // Windows: the pipe's name is also the lock against a second copy. Two copies started in the
    // same moment both pass the check above; the one that didn't get the pipe leaves here, before
    // it starts a second Electron on the same sign-in profile.
    #[cfg(windows)]
    if _ctl.is_none() && !test_instance && ctl::other_copy_running(!hidden) {
        eprintln!("uzumaki: already running{}", if hidden { "" } else { "; showed its window" });
        return;
    }

    // No library on disk (first start, or the library cache was cleared): the intro waits for it.
    let needs_sync = cache::Cache::default_location().needs_sync();
    #[cfg(target_os = "macos")]
    if login_check {
        let h = handle.clone();
        login::watch_launch(move |at_login| {
            eprintln!("uzumaki: {}", if at_login { "started at login, in the background" } else { "not a login start: opening the window" });
            h.update(move |s: &mut State, _| s.login_launch(at_login, needs_sync));
        });
    }
    // A new install gets the onboarding instead of the intro (`onboarding::at_launch` says who).
    // `UZUMAKI_ONBOARDING=1` shows it anyway, for a look on a scratch config.
    let forced = std::env::var_os("UZUMAKI_ONBOARDING").is_some_and(|v| v == "1");
    let fresh = cache::Cache::default_location().fresh();
    let ui_engine = engine.clone();
    let ui_handle = handle.clone();
    let app = app.init(move || {
        let mut s = State::new(None, Some(local), Some(ui_handle));
        s.engine = Some(ui_engine);
        s.load_local();
        s.background = hidden;
        // Start at login as the system has it; an autostart entry left pointing at a removed
        // program (a replaced AppImage) is pointed at this copy first.
        if !test_instance {
            login::repair();
        }
        s.read_login(login::read());
        // `--login` also keeps Sign in on the bridge's full window (a debug path).
        s.session.debug_login = debug_login;
        // A `--hidden` start sets the onboarding up too: it shows when the window first opens.
        // A new install starts no bridge until Apple Music is chosen (or the tour skipped).
        let onboarding = ui::onboarding::at_launch(s.settings.onboarded, fresh, test_instance, forced);
        ui::onboarding::launch(&mut s, onboarding, hidden, needs_sync, show_login);
        // At most once a day, off the UI thread (`update.rs`); an hourly timer repeats it while
        // the app stays open.
        s.start_update_check();
        s.start_update_timer();
        // The last run's song, paused where it was left (`resume.rs`); also arms saving it.
        s.start_resume();
        // Settings → Speakers → "Reconnect to the last speaker" (off unless chosen).
        s.airplay_reconnect();
        s
    });
    // Same as `app.run()` (one window, "main"), but through the display-aware entry point so the
    // background picture can be rendered once at screen size (`backdrop::set_screens`).
    rune::run_windows(move |displays| {
        backdrop::set_screens(displays);
        [rune::WindowSpec::from_app("main", app)]
    });

    // The playback to resume, before Electron goes (the atexit handler writes it too).
    resume::flush();
    engine.shutdown();
    airplay::shutdown(); // stop the speaker and its helper
    discord::shutdown(); // clear the card on Discord
}
