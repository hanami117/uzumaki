//! The macOS menu bar icon: an `NSStatusItem` with Play/Pause, Previous, Next, a volume slider,
//! Show and Quit, the current song as a disabled first item and as the tooltip. A click opens the
//! menu, as for every menu bar item. Rune has no Mac tray, so this talks to AppKit itself.
//!
//! AppKit objects live on the main thread. `TrayCtl` is built and used from the UI thread (the
//! state's init and its update handlers run there, and Rune's UI thread is the main thread on
//! macOS); `MainThreadMarker::new()` fails anywhere else and `start` then returns None. Menu
//! actions go to the app through the handle, like the Linux tray.
//!
//! The volume item is a custom view, like the Sound menu's: where the sound goes ("This Mac",
//! "Bedroom", a Chromecast) over a continuous `NSSlider` (accessibility label "Volume"). It moves
//! what the window's control for that output moves (`State::set_output_volume`). A drag sends at
//! most one value per `output_volume::THROTTLE` (a timer sends the last one held back); the mouse
//! up and the menu closing end the drag. Each time the menu opens, and every `SYNC_EVERY` ticks
//! while it is open, the state is asked for the current output and volume (`sync`), so changes
//! made elsewhere (window, `uzu vol`, media keys, the speakers) show; the knob is left alone for
//! `HOLD` after the user's own move. Handler updates run during the menu's tracking: winit's
//! wake-up source is in the common run loop modes, and so is the tick timer.
//!
//! While the menu is open it isn't rebuilt (a song change mid-drag would pull the slider away);
//! a change waits for it to close.
//!
//! Shuffle, Repeat (a submenu: Off, All, One) and AutoPlay sit after Next, with a checkmark on what
//! is on, as in Apple Music's Controls menu. They call what the window's buttons call
//! (`toggle_shuffle`, `set_repeat`, `toggle_autoplay`) and show `State::tray_modes`: when the menu
//! opens, on its sync ticks and whenever one changes. They are greyed while no song is loaded (the
//! window's buttons aren't there then), and AutoPlay also where the window has no ∞ button.
//!
//! The icon is always a template image: black with alpha, derived from the app's tray swirl, so
//! macOS tints it for light and dark menu bars. It does not follow Settings → Appearance → App
//! icon (the menu bar keeps the plain swirl); the Dock and the bundle do.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, sel, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAccessibility, NSApplication, NSAutoresizingMaskOptions, NSColor, NSEventType, NSFont, NSImage, NSImageView, NSLineBreakMode, NSMenu,
    NSMenuDelegate, NSMenuItem, NSSlider, NSStatusBar, NSStatusItem, NSTextField, NSVariableStatusItemLength, NSView,
};
use objc2_app_kit::{NSControlStateValueOff, NSControlStateValueOn};
use objc2_foundation::{NSData, NSObject, NSObjectProtocol, NSPoint, NSRect, NSRunLoop, NSRunLoopCommonModes, NSSize, NSString, NSTimer};

use crate::models::Song;
use crate::settings::IconVariant;
use crate::state::output_volume::{Output, OutputVolume, Throttle};
use crate::state::tray_modes::{repeat_word, Modes};
use crate::state::Repeat;
use crate::state::{Handle, State};

/// Menu bar icon height in points; the image is twice that in pixels for Retina.
const ICON_PT: f64 = 18.0;
const ICON_PX: u32 = 36;

const PLAY_PAUSE: isize = 1;
const PREVIOUS: isize = 2;
const NEXT: isize = 3;
const SHOW: isize = 4;
const QUIT: isize = 5;
const CHECK_UPDATES: isize = 6;
const SHUFFLE: isize = 7;
const AUTOPLAY: isize = 8;
/// The Repeat submenu's items (the parent has no action).
const REPEAT_OFF: isize = 10;
const REPEAT_ALL: isize = 11;
const REPEAT_ONE: isize = 12;
const REPEATS: [(isize, Repeat); 3] = [(REPEAT_OFF, Repeat::Off), (REPEAT_ALL, Repeat::All), (REPEAT_ONE, Repeat::One)];

/// The volume item's view, in points (the menu stretches it to its own width).
const VOLUME_W: f64 = 240.0;
const VOLUME_H: f64 = 46.0;
/// Left and right margin, lined up with the menu's item titles.
const INSET: f64 = 14.0;
/// The timer while the menu is open: sends a held-back value, and syncs every `SYNC_EVERY`.
const TICK_S: f64 = 0.1;
const SYNC_EVERY: u32 = 4;
/// After the user's own move, a sync doesn't move the knob for this long (late echoes).
const HOLD: Duration = Duration::from_secs(1);

/// The slider's drag, as the target sees it.
#[derive(Default)]
struct Slide {
    throttle: Throttle,
    /// The last value the slider gave.
    last: f64,
    /// A drag began and its end (`done`) wasn't sent yet.
    open: bool,
    /// When the user last moved it.
    at: Option<Instant>,
}

struct Ivars {
    handle: Handle,
    slide: RefCell<Slide>,
    /// The menu is open.
    open: Cell<bool>,
    ticks: Cell<u32>,
    timer: RefCell<Option<Retained<NSTimer>>>,
}

define_class!(
    // The target of every menu item (the item's tag says which one was chosen), of the volume
    // slider, of its timer, and the menu's delegate.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "UzumakiTrayTarget"]
    #[ivars = Ivars]
    struct Target;

    impl Target {
        #[unsafe(method(trayAction:))]
        fn tray_action(&self, item: &NSMenuItem) {
            let handle = &self.ivars().handle;
            match item.tag() {
                PLAY_PAUSE => handle.update(|s: &mut State, _| s.toggle_play()),
                PREVIOUS => handle.update(|s: &mut State, _| s.previous()),
                NEXT => handle.update(|s: &mut State, _| s.next()),
                SHUFFLE => handle.update(|s: &mut State, _| s.toggle_shuffle()),
                AUTOPLAY => handle.update(|s: &mut State, _| s.toggle_autoplay()),
                tag @ (REPEAT_OFF | REPEAT_ALL | REPEAT_ONE) => {
                    if let Some((_, mode)) = REPEATS.iter().find(|(t, _)| *t == tag) {
                        let mode = *mode;
                        handle.update(move |s: &mut State, _| s.set_repeat(mode));
                    }
                }
                SHOW => super::show_window(handle),
                // The answer shows in the window (the notice, or a toast).
                CHECK_UPDATES => {
                    super::show_window(handle);
                    handle.update(|s: &mut State, _| s.check_updates_now());
                }
                QUIT => handle.quit(),
                _ => {}
            }
        }

        #[unsafe(method(volumeSlid:))]
        fn volume_slid(&self, slider: &NSSlider) {
            let v = slider.doubleValue();
            let kind = NSApplication::sharedApplication(MainThreadMarker::from(self)).currentEvent().map(|e| e.r#type());
            // A mouse drag goes on; the mouse up, a click on the track's end or VoiceOver's
            // increment is a whole change.
            let dragging = matches!(kind, Some(NSEventType::LeftMouseDown | NSEventType::LeftMouseDragged));
            let now = Instant::now();
            let send = {
                let mut sl = self.ivars().slide.borrow_mut();
                sl.last = v;
                sl.at = Some(now);
                sl.open = dragging;
                sl.throttle.push(v, now, !dragging)
            };
            if let Some(v) = send {
                self.send(v, !dragging);
            }
        }

        #[unsafe(method(volumeTick:))]
        fn volume_tick(&self, _timer: &NSTimer) {
            let held = self.ivars().slide.borrow_mut().throttle.flush(Instant::now(), false);
            if let Some(v) = held {
                self.send(v, false);
            }
            let n = self.ivars().ticks.get().wrapping_add(1);
            self.ivars().ticks.set(n);
            if n.is_multiple_of(SYNC_EVERY) {
                self.request_sync();
            }
        }
    }

    unsafe impl NSObjectProtocol for Target {}

    unsafe impl NSMenuDelegate for Target {
        #[unsafe(method(menuWillOpen:))]
        fn menu_will_open(&self, _menu: &NSMenu) {
            self.ivars().open.set(true);
            self.ivars().ticks.set(0);
            self.request_sync();
            let mut timer = self.ivars().timer.borrow_mut();
            if timer.is_none() {
                let target: &AnyObject = self;
                let t = unsafe { NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(TICK_S, target, sel!(volumeTick:), None, true) };
                // Common modes: it also fires while the menu tracks the mouse.
                unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&t, NSRunLoopCommonModes) };
                *timer = Some(t);
            }
        }

        #[unsafe(method(menuDidClose:))]
        fn menu_did_close(&self, _menu: &NSMenu) {
            self.ivars().open.set(false);
            self.stop_timer();
            // The drag's last value, and its end (AirPlay saves the speakers' volumes).
            let last = {
                let mut sl = self.ivars().slide.borrow_mut();
                let held = sl.throttle.flush(Instant::now(), true);
                let open = std::mem::take(&mut sl.open);
                if held.is_some() || open { Some(held.unwrap_or(sl.last)) } else { None }
            };
            if let Some(v) = last {
                self.send(v, true);
            }
            self.ivars().handle.update_if(|s: &mut State, _| {
                if let Some(t) = &mut s.tray {
                    t.menu_closed();
                }
                false
            });
        }
    }
);

impl Target {
    fn send(&self, v: f64, done: bool) {
        self.ivars().handle.update(move |s: &mut State, _| s.set_output_volume(v, done));
    }

    /// Asks the state for where the sound goes and its volume (nothing to redraw in the window).
    fn request_sync(&self) {
        self.ivars().handle.update_if(|s: &mut State, _| {
            sync(s);
            false
        });
    }

    fn stop_timer(&self) {
        if let Some(t) = self.ivars().timer.borrow_mut().take() {
            t.invalidate();
        }
    }

    fn recently_slid(&self) -> bool {
        self.ivars().slide.borrow().at.is_some_and(|t| t.elapsed() < HOLD)
    }
}

/// Shows the state's output and volume on the slider.
fn sync(s: &mut State) {
    let o = s.output_volume();
    let modes = s.tray_modes();
    if let Some(t) = &mut s.tray {
        t.show_output(&o);
        t.set_modes(&modes);
    }
}

/// The volume item and the views it updates.
struct VolumeItem {
    item: Retained<NSMenuItem>,
    slider: Retained<NSSlider>,
    label: Retained<NSTextField>,
    icon: Retained<NSImageView>,
}

/// Shuffle, Repeat and AutoPlay. Like the volume item, they move to each new menu.
struct ModeItems {
    shuffle: Retained<NSMenuItem>,
    repeat: Retained<NSMenuItem>,
    repeats: Vec<Retained<NSMenuItem>>,
    autoplay: Retained<NSMenuItem>,
}

pub struct TrayCtl {
    item: Retained<NSStatusItem>,
    target: Retained<Target>,
    mtm: MainThreadMarker,
    playing: bool,
    /// "Title — Artist" or empty.
    song: String,
    menu: Option<Retained<NSMenu>>,
    volume: VolumeItem,
    modes: ModeItems,
    /// What the three mode items show.
    shown_modes: Option<Modes>,
    /// A song is shown: Shuffle and Repeat (and AutoPlay) can be used.
    loaded: bool,
    /// What the volume item shows (None: not synced yet, or the knob was held).
    shown: Option<OutputVolume>,
    /// A change came while the menu was open: rebuild once it closes.
    stale: bool,
}

impl TrayCtl {
    /// None when not on the main thread (AppKit would crash there), and in headless runs.
    pub fn start(handle: Handle, app_icon: IconVariant) -> Option<Self> {
        if rune::is_headless() {
            return None;
        }
        let Some(mtm) = MainThreadMarker::new() else {
            eprintln!("uzumaki: no menu bar icon: not on the main thread");
            return None;
        };
        let target = mtm.alloc::<Target>().set_ivars(Ivars {
            handle,
            slide: RefCell::default(),
            open: Cell::new(false),
            ticks: Cell::new(0),
            timer: RefCell::new(None),
        });
        let target: Retained<Target> = unsafe { msg_send![super(target), init] };
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        if let Some(button) = item.button(mtm) {
            button.setToolTip(Some(&NSString::from_str("Uzumaki")));
        }
        let volume = volume_item(mtm, &target);
        let modes = mode_items(mtm, &target);
        let mut ctl = Self {
            item,
            target,
            mtm,
            playing: false,
            song: String::new(),
            menu: None,
            volume,
            modes,
            shown_modes: None,
            loaded: false,
            shown: None,
            stale: false,
        };
        ctl.set_app_icon(app_icon);
        ctl.rebuild();
        // The slider shows the volume before the menu first opens.
        ctl.target.request_sync();
        Some(ctl)
    }

    /// Shows the template swirl. The app icon choice is ignored here (see the module docs).
    pub fn set_app_icon(&mut self, _icon: IconVariant) {
        let Some(button) = self.item.button(self.mtm) else { return };
        if let Some(image) = template_image() {
            button.setTitle(&NSString::new());
            button.setImage(Some(&image));
        } else {
            button.setTitle(&NSString::from_str("U")); // the icon failed to decode
        }
    }

    /// Relabels Play/Pause.
    pub fn set_playing(&mut self, playing: bool) {
        if playing != self.playing {
            self.playing = playing;
            self.changed();
        }
    }

    /// The first menu item and the tooltip: "Title — Artist", or just "Uzumaki".
    pub fn set_song(&mut self, song: Option<&Song>) {
        let text = match song {
            Some(s) if !s.artist.is_empty() => format!("{} — {}", s.title, s.artist),
            Some(s) => s.title.clone(),
            None => String::new(),
        };
        if song.is_some() != self.loaded {
            self.loaded = song.is_some();
            self.apply_modes();
        }
        if text != self.song {
            if let Some(button) = self.item.button(self.mtm) {
                let tip = if text.is_empty() { "Uzumaki" } else { &text };
                button.setToolTip(Some(&NSString::from_str(tip)));
            }
            self.song = text;
            self.changed();
        }
    }

    /// The output's name, icon and volume on the slider. The knob stays where the user just put it.
    fn show_output(&mut self, o: &OutputVolume) {
        if self.shown.as_ref() == Some(o) {
            return;
        }
        let was = self.shown.take();
        let v = &self.volume;
        if was.as_ref().map(|w| &w.label) != Some(&o.label) {
            v.label.setStringValue(&NSString::from_str(&o.label));
        }
        if was.as_ref().map(|w| w.output) != Some(o.output)
            && let Some(image) = output_symbol(o.output)
        {
            v.icon.setImage(Some(&image));
        }
        v.slider.setEnabled(o.volume.is_some());
        let held = self.target.recently_slid();
        if let (Some(x), false) = (o.volume, held) {
            v.slider.setDoubleValue(x);
        }
        // A held knob syncs again next time, even if the state stays the same.
        if !held {
            self.shown = Some(o.clone());
        }
    }

    /// Checkmarks and greying of Shuffle, Repeat and AutoPlay (in place, also while the menu is open).
    pub fn set_modes(&mut self, modes: &Modes) {
        if self.shown_modes.as_ref() != Some(modes) {
            self.shown_modes = Some(*modes);
            self.apply_modes();
        }
    }

    fn apply_modes(&self) {
        let Some(m) = &self.shown_modes else {
            return;
        };
        let it = &self.modes;
        let mark = |on: bool| if on { NSControlStateValueOn } else { NSControlStateValueOff };
        it.shuffle.setState(mark(m.shuffle));
        it.shuffle.setEnabled(self.loaded);
        it.repeat.setEnabled(self.loaded);
        for (item, (_, mode)) in it.repeats.iter().zip(REPEATS) {
            item.setState(mark(m.repeat == mode));
        }
        it.autoplay.setState(mark(m.autoplay));
        it.autoplay.setEnabled(m.autoplay_enabled(self.loaded));
    }

    /// The menu closed: a change it held back applies now.
    fn menu_closed(&mut self) {
        if self.stale {
            self.rebuild();
        }
    }

    fn changed(&mut self) {
        if self.target.ivars().open.get() {
            self.stale = true;
        } else {
            self.rebuild();
        }
    }

    fn rebuild(&mut self) {
        // The volume item moves to the new menu (an item can be in one menu only).
        if let Some(old) = self.menu.take() {
            for item in [&self.volume.item, &self.modes.shuffle, &self.modes.repeat, &self.modes.autoplay] {
                if unsafe { item.menu() }.is_some() {
                    old.removeItem(item);
                }
            }
        }
        let menu = NSMenu::new(self.mtm);
        // Without this AppKit would grey the song line and enable the rest by itself; be explicit.
        menu.setAutoenablesItems(false);
        if !self.song.is_empty() {
            let title = NSMenuItem::new(self.mtm);
            title.setTitle(&NSString::from_str(&self.song));
            title.setEnabled(false);
            menu.addItem(&title);
            menu.addItem(&NSMenuItem::separatorItem(self.mtm));
        }
        self.add(&menu, if self.playing { "Pause" } else { "Play" }, PLAY_PAUSE);
        self.add(&menu, "Previous", PREVIOUS);
        self.add(&menu, "Next", NEXT);
        menu.addItem(&NSMenuItem::separatorItem(self.mtm));
        menu.addItem(&self.modes.shuffle);
        menu.addItem(&self.modes.repeat);
        menu.addItem(&self.modes.autoplay);
        menu.addItem(&NSMenuItem::separatorItem(self.mtm));
        menu.addItem(&self.volume.item);
        menu.addItem(&NSMenuItem::separatorItem(self.mtm));
        self.add(&menu, "Show Uzumaki", SHOW);
        self.add(&menu, "Check for Updates\u{2026}", CHECK_UPDATES);
        menu.addItem(&NSMenuItem::separatorItem(self.mtm));
        self.add(&menu, "Quit Uzumaki", QUIT);
        menu.setDelegate(Some(ProtocolObject::from_ref(&*self.target)));
        self.item.setMenu(Some(&menu));
        self.menu = Some(menu);
        self.stale = false;
        self.apply_modes();
    }

    fn add(&self, menu: &NSMenu, title: &str, tag: isize) {
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                self.mtm.alloc::<NSMenuItem>(),
                &NSString::from_str(title),
                Some(sel!(trayAction:)),
                &NSString::new(),
            )
        };
        item.setTag(tag);
        let target: &AnyObject = &self.target;
        unsafe { item.setTarget(Some(target)) };
        menu.addItem(&item);
    }
}

/// A menu item that calls `trayAction:` on the target.
fn action_item(mtm: MainThreadMarker, target: &Target, title: &str, tag: isize) -> Retained<NSMenuItem> {
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(mtm.alloc::<NSMenuItem>(), &NSString::from_str(title), Some(sel!(trayAction:)), &NSString::new())
    };
    item.setTag(tag);
    let target: &AnyObject = target;
    unsafe { item.setTarget(Some(target)) };
    item
}

/// Shuffle, Repeat with its Off / All / One submenu, and AutoPlay. Greyed until the first sync.
fn mode_items(mtm: MainThreadMarker, target: &Target) -> ModeItems {
    let shuffle = action_item(mtm, target, "Shuffle", SHUFFLE);
    let autoplay = action_item(mtm, target, "AutoPlay", AUTOPLAY);
    let repeat = NSMenuItem::new(mtm);
    repeat.setTitle(&NSString::from_str("Repeat"));
    let sub = NSMenu::new(mtm);
    sub.setAutoenablesItems(false);
    let repeats: Vec<_> = REPEATS.iter().map(|&(tag, mode)| action_item(mtm, target, repeat_word(mode), tag)).collect();
    for r in &repeats {
        sub.addItem(r);
    }
    repeat.setSubmenu(Some(&sub));
    for i in [&shuffle, &repeat, &autoplay] {
        i.setEnabled(false);
    }
    ModeItems { shuffle, repeat, repeats, autoplay }
}

/// The SF Symbol for where the sound goes.
fn output_symbol(output: Output) -> Option<Retained<NSImage>> {
    let name = symbol_name(output);
    NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(name), None)
}

fn symbol_name(output: Output) -> &'static str {
    match output {
        Output::ThisComputer => "speaker.wave.2.fill",
        Output::AirPlay => "airplayaudio",
        Output::Cast => "tv",
    }
}

/// The volume item: an icon and the output's name over a full-width slider.
fn volume_item(mtm: MainThreadMarker, target: &Target) -> VolumeItem {
    let rect = |x: f64, y: f64, w: f64, h: f64| NSRect::new(NSPoint::new(x, y), NSSize::new(w, h));
    let wide = NSAutoresizingMaskOptions::ViewWidthSizable;
    let view = NSView::initWithFrame(mtm.alloc::<NSView>(), rect(0.0, 0.0, VOLUME_W, VOLUME_H));
    view.setAutoresizingMask(wide);

    let placeholder = output_symbol(Output::ThisComputer).unwrap_or_default();
    let icon = NSImageView::imageViewWithImage(&placeholder, mtm);
    icon.setFrame(rect(INSET, 26.0, 16.0, 16.0));
    view.addSubview(&icon);

    let label = NSTextField::labelWithString(&NSString::new(), mtm);
    label.setFrame(rect(INSET + 22.0, 25.0, VOLUME_W - 2.0 * INSET - 22.0, 17.0));
    label.setFont(Some(&NSFont::systemFontOfSize(12.0)));
    label.setTextColor(Some(&NSColor::secondaryLabelColor()));
    label.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    label.setAutoresizingMask(wide);
    view.addSubview(&label);

    let target: &AnyObject = target;
    let slider = unsafe { NSSlider::sliderWithValue_minValue_maxValue_target_action(0.0, 0.0, 1.0, Some(target), Some(sel!(volumeSlid:)), mtm) };
    slider.setFrame(rect(INSET, 4.0, VOLUME_W - 2.0 * INSET, 20.0));
    slider.setContinuous(true);
    slider.setAutoresizingMask(wide);
    slider.setAccessibilityLabel(Some(&NSString::from_str("Volume")));
    view.addSubview(&slider);

    let item = NSMenuItem::new(mtm);
    item.setView(Some(&view));
    VolumeItem { item, slider, label, icon }
}

impl Drop for TrayCtl {
    /// Removes the icon from the menu bar (and stops the timer, which holds the target).
    fn drop(&mut self) {
        self.target.stop_timer();
        NSStatusBar::systemStatusBar().removeStatusItem(&self.item);
    }
}

fn template_image() -> Option<Retained<NSImage>> {
    let (w, h, rgba) = template_pixels()?;
    let img = image::RgbaImage::from_raw(w, h, rgba)?;
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
    let image = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(&png))?;
    // The pixels are for 2x, so the point size is half and it stays sharp on Retina.
    image.setSize(NSSize::new(ICON_PT * f64::from(w) / f64::from(h), ICON_PT));
    image.setTemplate(true);
    Some(image)
}

/// The 64 px tray PNG of the Dark app icon (the same file as the Linux tray).
const TRAY_PNG: &[u8] = include_bytes!("../../assets/icons/variants/dark/tray.png");

/// The swirl of the tray PNG as black with alpha, cropped to its bounds and `ICON_PX` tall.
fn template_pixels() -> Option<(u32, u32, Vec<u8>)> {
    let src = image::load_from_memory_with_format(TRAY_PNG, image::ImageFormat::Png).ok()?.to_rgba8();
    Some(template_from(&src))
}

/// Red of the dark tile (#1d1d22) and of the coral swirl (#ff7a8a). Red runs between them
/// linearly across an antialiased edge, so it measures how much swirl a pixel has; the tile
/// and its rounded corners end up with no alpha.
const TILE_RED: f32 = 29.0;
const CORAL_RED: f32 = 255.0;

fn template_from(src: &image::RgbaImage) -> (u32, u32, Vec<u8>) {
    let mut mask = image::RgbaImage::new(src.width(), src.height());
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for (x, y, p) in src.enumerate_pixels() {
        let swirl = ((f32::from(p[0]) - TILE_RED) / (CORAL_RED - TILE_RED)).clamp(0.0, 1.0);
        let a = (swirl * f32::from(p[3])).round() as u8;
        mask.put_pixel(x, y, image::Rgba([0, 0, 0, a]));
        if a > 40 {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
    }
    if x0 > x1 {
        return (1, 1, vec![0; 4]); // no swirl found
    }
    let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
    let cropped = image::imageops::crop_imm(&mask, x0, y0, w, h).to_image();
    // A square canvas so the swirl isn't stretched, then the menu bar size.
    let side = w.max(h);
    let mut square = image::RgbaImage::new(side, side);
    image::imageops::replace(&mut square, &cropped, i64::from((side - w) / 2), i64::from((side - h) / 2));
    let out = image::imageops::resize(&square, ICON_PX, ICON_PX, image::imageops::FilterType::Lanczos3);
    (ICON_PX, ICON_PX, out.into_raw())
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_template_is_a_black_swirl_without_a_tile() {
        let (w, h, rgba) = super::template_pixels().expect("the tray PNG decodes");
        assert_eq!((w, h), (super::ICON_PX, super::ICON_PX));
        assert!(rgba.chunks(4).all(|p| p[..3] == [0, 0, 0]), "black only; macOS tints it");
        let alpha = |x: u32, y: u32| rgba[((y * w + x) * 4 + 3) as usize];
        // The corners were the tile's: no opaque background is left.
        for (x, y) in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
            assert_eq!(alpha(x, y), 0, "transparent corner");
        }
        let solid = rgba.chunks(4).filter(|p| p[3] > 200).count();
        let clear = rgba.chunks(4).filter(|p| p[3] < 20).count();
        assert!(solid > 100, "a swirl, got {solid} solid pixels");
        assert!(clear > 200, "and not a filled square, got {clear} clear pixels");
    }

    #[test]
    fn every_output_has_its_symbol() {
        use crate::state::output_volume::Output;
        for o in [Output::ThisComputer, Output::AirPlay, Output::Cast] {
            assert!(super::output_symbol(o).is_some(), "SF Symbol {} on this macOS", super::symbol_name(o));
        }
    }

    #[test]
    fn the_template_comes_from_the_dark_tray_png() {
        assert_eq!(super::TRAY_PNG, include_bytes!("../../assets/icons/uzumaki-tray.png"));
    }
}
