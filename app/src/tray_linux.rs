//! The system tray icon on Linux: a StatusNotifierItem with a `com.canonical.dbusmenu` menu, through
//! ksni (on the zbus the app already has for MPRIS). The menu is `tray_menu.rs`'s, as the macOS
//! menu bar's: the song, Play/Pause, Previous, Next, Shuffle and AutoPlay as checkmarks (dbusmenu
//! `toggle-type` "checkmark"), Repeat as a submenu of radio items, a Volume submenu, Show, Check for
//! Updates and Quit. A left click shows the window; scrolling over the icon changes the volume of
//! where the sound goes, as KDE's media players do. The tooltip is the song.
//!
//! Threads: ksni serves D-Bus on its own thread, and its callbacks (`Sni`) only post to the UI
//! thread (`handle.update`). The UI thread never waits for the tray: `TrayCtl`'s setters send the
//! new `Snapshot` to a small worker thread, which hands the latest one to ksni (`Handle::update`
//! waits for the D-Bus thread). ksni diffs the menu and emits `LayoutUpdated` /
//! `ItemsPropertiesUpdated` (and `NewToolTip`, `NewIcon`), so an open menu follows too (KDE does).
//!
//! Freshness: the modes arrive whenever they change (`sync_tray_modes`). The output's volume is
//! asked for each time the menu opens (`AboutToShow`; the answer follows a moment later as an
//! update) and after the tray's own changes.
//!
//! It shows only where a tray host runs (KDE, waybar, XFCE and most bars; GNOME needs the
//! AppIndicator extension). With no host yet (Plasma's panel starting after the app at login) it
//! waits and registers when one comes, as Rune's tray did.

use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ksni::blocking::TrayMethods;
use ksni::menu::{CheckmarkItem, RadioGroup, RadioItem, StandardItem, SubMenu};
use ksni::{MenuItem, Orientation};

use super::icon::{icon_pixels, ICON_PX};
use super::menu::{self, entries, Cmd, Entry, ScrollSteps, Snapshot, TRAY_STEP};
use crate::models::Song;
use crate::settings::IconVariant;
use crate::state::output_volume::OutputVolume;
use crate::state::tray_modes::Modes;
use crate::state::{Handle, State};

/// A scroll ends after this long without another notch: AirPlay then saves the speakers' volumes.
const SCROLL_END: Duration = Duration::from_millis(600);

/// What the tray hands to the app.
#[derive(Clone, Debug, PartialEq)]
enum Ev {
    Cmd(Cmd),
    /// A left click, with the host's XDG activation token when it gave one.
    Activate(Option<String>),
    /// Volume steps from a scroll on the icon (louder positive).
    Scroll(i32),
    ScrollEnd,
    /// The menu is opening: the tray would like the current volume.
    AboutToShow,
}

type OnEvent = Arc<dyn Fn(Ev) + Send + Sync>;

/// To the worker thread.
enum Msg {
    Snap(Snapshot),
    Icon(Vec<ksni::Icon>),
    Scrolled,
    Stop,
}

/// What ksni serves.
struct Sni {
    snap: Snapshot,
    icon: Vec<ksni::Icon>,
    scroll: ScrollSteps,
    on_event: OnEvent,
    worker: mpsc::Sender<Msg>,
}

impl ksni::Tray for Sni {
    fn id(&self) -> String {
        "uzumaki".into()
    }

    fn title(&self) -> String {
        "Uzumaki".into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icon.clone()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip { title: self.snap.tooltip().to_string(), ..Default::default() }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        (self.on_event)(Ev::Activate(ksni::current_xdg_activation_token()));
    }

    fn scroll(&mut self, delta: i32, orientation: Orientation) {
        if !matches!(orientation, Orientation::Vertical) {
            return;
        }
        let steps = self.scroll.push(delta, Instant::now());
        if steps != 0 {
            (self.on_event)(Ev::Scroll(steps));
            let _ = self.worker.send(Msg::Scrolled);
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        items(&entries(&self.snap))
    }

    fn menu_about_to_show(&mut self) {
        (self.on_event)(Ev::AboutToShow);
    }

    /// No host (yet): keep serving, ksni registers when a watcher appears.
    fn watcher_offline(&self, _reason: ksni::OfflineReason) -> bool {
        true
    }
}

/// dbusmenu labels treat `_` as the access-key mark: a song's own underscores are doubled.
fn label(text: &str) -> String {
    text.replace('_', "__")
}

/// `tray_menu`'s entries as ksni items; each one posts its command.
fn items(entries: &[Entry]) -> Vec<MenuItem<Sni>> {
    let post = |cmd: Cmd| Box::new(move |t: &mut Sni| (t.on_event)(Ev::Cmd(cmd)));
    entries
        .iter()
        .map(|e| match e {
            Entry::Label(text) => StandardItem { label: label(text), enabled: false, ..Default::default() }.into(),
            Entry::Action { label: text, cmd, enabled } => {
                StandardItem { label: label(text), enabled: *enabled, activate: post(*cmd), ..Default::default() }.into()
            }
            Entry::Check { label: text, cmd, checked, enabled } => {
                CheckmarkItem { label: label(text), enabled: *enabled, checked: *checked, activate: post(*cmd), ..Default::default() }.into()
            }
            Entry::Radio { options, selected, enabled } => {
                let cmds: Vec<Cmd> = options.iter().map(|o| o.1).collect();
                RadioGroup {
                    // Out of range: none marked (a volume not known yet).
                    selected: selected.unwrap_or(usize::MAX),
                    select: Box::new(move |t: &mut Sni, i: usize| {
                        if let Some(&cmd) = cmds.get(i) {
                            (t.on_event)(Ev::Cmd(cmd));
                        }
                    }),
                    options: options.iter().map(|(text, _)| RadioItem { label: label(text), enabled: *enabled, ..Default::default() }).collect(),
                }
                .into()
            }
            Entry::Submenu { label: text, enabled, items: sub } => {
                SubMenu { label: label(text), enabled: *enabled, submenu: items(sub), ..Default::default() }.into()
            }
            Entry::Separator => MenuItem::Separator,
        })
        .collect()
}

/// ARGB32 in network byte order, as `IconPixmap` wants.
fn pixmaps(app_icon: IconVariant) -> Vec<ksni::Icon> {
    let rgba = icon_pixels(app_icon, ICON_PX);
    let data = rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[3], p[0], p[1], p[2]]).collect();
    vec![ksni::Icon { width: ICON_PX as i32, height: ICON_PX as i32, data }]
}

/// The served item and its worker thread. Dropping it removes the icon.
struct Service {
    tx: mpsc::Sender<Msg>,
    worker: Option<JoinHandle<()>>,
}

impl Service {
    /// `kde`: the host is KDE Plasma's (its Scroll deltas are Qt's, see `ScrollSteps`).
    fn spawn(snap: Snapshot, icon: Vec<ksni::Icon>, kde: bool, on_event: OnEvent) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel();
        let sni = Sni { snap, icon, scroll: ScrollSteps::new(kde), on_event: on_event.clone(), worker: tx.clone() };
        let handle = sni.assume_sni_available(true).spawn().map_err(|e| e.to_string())?;
        let worker = std::thread::Builder::new()
            .name("uzumaki-tray".into())
            .spawn(move || work(&handle, &rx, &on_event))
            .map_err(|e| e.to_string())?;
        Ok(Self { tx, worker: Some(worker) })
    }

    fn send(&self, msg: Msg) {
        let _ = self.tx.send(msg);
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Stop);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

/// The worker: hands the latest snapshot and icon to ksni (several queued ones become one
/// update), and ends a scroll once it has been quiet for `SCROLL_END`.
fn work(handle: &ksni::blocking::Handle<Sni>, rx: &mpsc::Receiver<Msg>, on_event: &OnEvent) {
    let mut scroll_end: Option<Instant> = None;
    let mut stop = false;
    while !stop {
        let first = match scroll_end {
            Some(at) => match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                Ok(m) => Some(m),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            },
            None => match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => break,
            },
        };
        let Some(first) = first else {
            scroll_end = None;
            on_event(Ev::ScrollEnd);
            continue;
        };
        let (mut snap, mut icon) = (None, None);
        for msg in std::iter::once(first).chain(std::iter::from_fn(|| rx.try_recv().ok())) {
            match msg {
                Msg::Snap(s) => snap = Some(s),
                Msg::Icon(i) => icon = Some(i),
                Msg::Scrolled => scroll_end = Some(Instant::now() + SCROLL_END),
                Msg::Stop => stop = true,
            }
        }
        if snap.is_some() || icon.is_some() {
            handle.update(move |t: &mut Sni| {
                if let Some(s) = snap {
                    t.snap = s;
                }
                if let Some(i) = icon {
                    t.icon = i;
                }
            });
        }
    }
    if scroll_end.is_some() {
        on_event(Ev::ScrollEnd);
    }
    handle.shutdown().wait();
}

/// KDE Plasma, from `XDG_CURRENT_DESKTOP` ("KDE"); its tray sends Qt's wheel deltas.
fn desktop_is_kde() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.split(':').any(|p| p.eq_ignore_ascii_case("kde")))
}

pub struct TrayCtl {
    service: Service,
    snap: Snapshot,
    icon: IconVariant,
}

impl TrayCtl {
    /// None when there is no session bus (or in headless runs).
    pub fn start(handle: Handle, app_icon: IconVariant) -> Option<Self> {
        if rune::is_headless() {
            return None;
        }
        let on_event: OnEvent = Arc::new(move |ev: Ev| match ev {
            Ev::Cmd(cmd) => menu::run(&handle, cmd),
            Ev::Activate(Some(token)) => handle.activate(token),
            Ev::Activate(None) => handle.open(),
            Ev::Scroll(steps) => handle.update(move |s: &mut State, _| {
                s.step_output_volume(steps * TRAY_STEP, false);
                s.sync_tray_modes();
            }),
            Ev::ScrollEnd => handle.update(|s: &mut State, _| {
                s.end_output_drag();
                s.sync_tray_modes();
            }),
            Ev::AboutToShow => handle.update_if(|s: &mut State, _| {
                s.sync_tray_modes();
                false // nothing to redraw in the window
            }),
        });
        Self::serve(app_icon, desktop_is_kde(), on_event)
            .map_err(|e| eprintln!("uzumaki: no tray icon: {e}"))
            .ok()
    }

    fn serve(app_icon: IconVariant, kde: bool, on_event: OnEvent) -> Result<Self, String> {
        let service = Service::spawn(Snapshot::default(), pixmaps(app_icon), kde, on_event)?;
        Ok(Self { service, snap: Snapshot::default(), icon: app_icon })
    }

    /// Swaps the icon for the chosen app icon's (hosts redraw it on `NewIcon`).
    pub fn set_app_icon(&mut self, icon: IconVariant) {
        if icon != self.icon {
            self.icon = icon;
            self.service.send(Msg::Icon(pixmaps(icon)));
        }
    }

    /// Relabels Play/Pause.
    pub fn set_playing(&mut self, playing: bool) {
        self.change(|s| s.playing = playing);
    }

    /// The menu's first line and the tooltip: "Title — Artist"; the modes are greyed with no song.
    pub fn set_song(&mut self, song: Option<&Song>) {
        let line = super::song_line(song);
        self.change(|s| {
            s.loaded = song.is_some();
            s.song = line;
        });
    }

    /// Checkmarks of Shuffle, Repeat and AutoPlay.
    pub fn set_modes(&mut self, modes: &Modes) {
        self.change(|s| s.modes = Some(*modes));
    }

    /// The Volume submenu: where the sound goes and its volume.
    pub fn set_output(&mut self, output: &OutputVolume) {
        self.change(|s| s.output = Some(output.clone()));
    }

    fn change(&mut self, f: impl FnOnce(&mut Snapshot)) {
        let mut next = self.snap.clone();
        f(&mut next);
        if next != self.snap {
            self.snap = next.clone();
            self.service.send(Msg::Snap(next));
        }
    }
}

/// The bus test runs on a private session bus only (`UZUMAKI_PRIVATE_BUS=1` under
/// `dbus-run-session`, as `scripts/linux-check/run.sh bus` does): a fake StatusNotifierWatcher takes
/// the item's registration, and a plain D-Bus client reads the menu, clicks items and scrolls, as a
/// tray host would. Never on the desktop's own bus (the test icon would show in the real tray).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::output_volume::Output;
    use crate::state::Repeat;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use zbus::blocking::Proxy;
    use zbus::zvariant::{OwnedValue, Value};

    #[test]
    fn labels_keep_their_underscores() {
        assert_eq!(label("snake_case_song"), "snake__case__song");
    }

    #[test]
    fn the_icon_is_argb_in_network_order() {
        let p = pixmaps(IconVariant::Dark);
        assert_eq!((p[0].width, p[0].height), (64, 64));
        let rgba = icon_pixels(IconVariant::Dark, ICON_PX);
        let i = (32 * 64 + 10) * 4;
        assert_eq!(&p[0].data[i..i + 4], &[rgba[i + 3], rgba[i], rgba[i + 1], rgba[i + 2]]);
    }

    /// The watcher a tray host runs: records who registered.
    struct Watcher {
        items: Arc<Mutex<Vec<String>>>,
    }

    #[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
    impl Watcher {
        fn register_status_notifier_item(&self, service: String) {
            self.items.lock().unwrap().push(service);
        }
        fn register_status_notifier_host(&self, _service: String) {}
        #[zbus(property)]
        fn registered_status_notifier_items(&self) -> Vec<String> {
            self.items.lock().unwrap().clone()
        }
        #[zbus(property)]
        fn is_status_notifier_host_registered(&self) -> bool {
            true
        }
        #[zbus(property)]
        fn protocol_version(&self) -> i32 {
            0
        }
    }

    type Node = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);
    type Pixmaps = Vec<(i32, i32, Vec<u8>)>;

    /// One menu node as the host sees it.
    #[derive(Debug)]
    struct Item {
        id: i32,
        props: HashMap<String, OwnedValue>,
        children: Vec<Item>,
    }

    impl Item {
        fn from(node: Node) -> Item {
            let (id, props, children) = node;
            let children = children.into_iter().map(|c| Item::from(Node::try_from(c).expect("a menu node"))).collect();
            Item { id, props, children }
        }
        fn str(&self, key: &str) -> Option<String> {
            self.props.get(key).and_then(|v| String::try_from(v.try_clone().ok()?).ok())
        }
        fn int(&self, key: &str) -> Option<i32> {
            self.props.get(key).and_then(|v| i32::try_from(v).ok())
        }
        fn label(&self) -> String {
            self.str("label").unwrap_or_default()
        }
        fn enabled(&self) -> bool {
            self.props.get("enabled").and_then(|v| bool::try_from(v).ok()).unwrap_or(true)
        }
        fn child(&self, label: &str) -> &Item {
            self.children.iter().find(|c| c.label().starts_with(label)).unwrap_or_else(|| panic!("no {label} under {}", self.label()))
        }
        fn outline(&self) -> Vec<String> {
            self.children.iter().map(|c| if c.str("type").as_deref() == Some("separator") { "—".into() } else { c.label() }).collect()
        }
    }

    fn private_bus() -> bool {
        std::env::var("UZUMAKI_PRIVATE_BUS").is_ok_and(|v| v == "1") && std::env::var("DBUS_SESSION_BUS_ADDRESS").is_ok()
    }

    fn wait_for<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(v) = f() {
                return v;
            }
            assert!(Instant::now() < until, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_tray_host_reads_the_menu_clicks_and_scrolls_on_a_private_bus() {
        if !private_bus() {
            eprintln!("not on a private session bus (UZUMAKI_PRIVATE_BUS=1 under dbus-run-session): skipping");
            return;
        }
        // The host's side: the watcher, on its own connection.
        let registered = Arc::new(Mutex::new(Vec::new()));
        let host = zbus::blocking::connection::Builder::session()
            .unwrap()
            .name("org.kde.StatusNotifierWatcher")
            .unwrap()
            .serve_at("/StatusNotifierWatcher", Watcher { items: registered.clone() })
            .unwrap()
            .build()
            .unwrap();

        let (events_tx, events) = mpsc::channel::<Ev>();
        let events_tx = Mutex::new(events_tx);
        let on_event: OnEvent = Arc::new(move |ev| {
            let _ = events_tx.lock().unwrap().send(ev);
        });
        let mut tray = TrayCtl::serve(IconVariant::Dark, true, on_event).expect("the tray starts");
        let name = wait_for("the item to register", || registered.lock().unwrap().first().cloned());
        assert!(name.starts_with("org.kde.StatusNotifierItem-"), "{name}");
        let next = |what: &str| events.recv_timeout(Duration::from_secs(3)).unwrap_or_else(|_| panic!("no event for {what}"));

        // No property cache: a host reads them again on NewToolTip / NewIcon, as here.
        let proxy = |path: &str, iface: &'static str| -> Proxy<'static> {
            zbus::blocking::proxy::Builder::new(&host)
                .destination(name.clone())
                .unwrap()
                .path(path.to_string())
                .unwrap()
                .interface(iface)
                .unwrap()
                .cache_properties(zbus::proxy::CacheProperties::No)
                .build()
                .unwrap()
        };
        let item = proxy("/StatusNotifierItem", "org.kde.StatusNotifierItem");
        assert_eq!(item.get_property::<String>("Id").unwrap(), "uzumaki");
        assert_eq!(item.get_property::<String>("Title").unwrap(), "Uzumaki");
        assert_eq!(item.get_property::<String>("Status").unwrap(), "Active");
        let icons: Pixmaps = item.get_property("IconPixmap").unwrap();
        assert_eq!((icons[0].0, icons[0].1, icons[0].2.len()), (64, 64, 64 * 64 * 4));
        let tip: (String, Pixmaps, String, String) = item.get_property("ToolTip").unwrap();
        assert_eq!(tip.2, "Uzumaki", "no song yet");
        let menu_path: zbus::zvariant::OwnedObjectPath = item.get_property("Menu").unwrap();
        let menu = proxy(menu_path.as_str(), "com.canonical.dbusmenu");
        let layout = || -> Item {
            let (_rev, root): (u32, Node) = menu.call("GetLayout", &(0i32, -1i32, Vec::<String>::new())).unwrap();
            Item::from(root)
        };

        // Before the first sync: everything that needs the player is greyed.
        let root = layout();
        assert_eq!(root.outline()[..3], ["Play", "Previous", "Next"]);
        assert!(!root.child("Shuffle").enabled() && !root.child("Repeat").enabled() && !root.child("Volume").enabled());

        // The app tells it the state (what `sync_tray_modes` and the song hooks do).
        let song = Song {
            id: "i.1".into(),
            catalog_id: None,
            title: "Snake_Song".into(),
            artist: "Artist".into(),
            album: "Album".into(),
            album_is_fallback: false,
            guessed: Default::default(),
            duration: 200.0,
            track_number: None,
            disc_number: None,
            explicit: false,
            artwork: None,
            playable: true,
            genre: None,
            year: None,
            album_id: None,
            artists: Vec::new(),
        };
        tray.set_song(Some(&song));
        tray.set_playing(true);
        tray.set_modes(&Modes { shuffle: false, repeat: Repeat::Off, autoplay: true, autoplay_offered: true });
        tray.set_output(&OutputVolume { output: Output::ThisComputer, label: "This Computer".into(), volume: Some(0.4) });
        let root = wait_for("the first sync", || {
            let r = layout();
            (r.outline().iter().any(|l| l == "Volume: 40%")).then_some(r)
        });
        assert_eq!(
            root.outline(),
            [
                "Snake__Song — Artist", "—", "Pause", "Previous", "Next", "—", "Shuffle", "Repeat", "AutoPlay", "—", "Volume: 40%", "—",
                "Show Uzumaki", "Check for Updates\u{2026}", "—", "Quit Uzumaki"
            ]
        );
        assert!(!root.children[0].enabled(), "the song line is greyed");
        let tip: (String, Pixmaps, String, String) = item.get_property("ToolTip").unwrap();
        assert_eq!(tip.2, "Snake_Song — Artist");
        let shuffle = root.child("Shuffle");
        assert_eq!((shuffle.str("toggle-type").as_deref(), shuffle.int("toggle-state"), shuffle.enabled()), (Some("checkmark"), Some(0), true));
        let autoplay = root.child("AutoPlay");
        assert_eq!((autoplay.str("toggle-type").as_deref(), autoplay.int("toggle-state")), (Some("checkmark"), Some(1)));
        let repeat = root.child("Repeat");
        assert_eq!(repeat.str("children-display").as_deref(), Some("submenu"));
        let marks: Vec<(String, Option<String>, Option<i32>)> =
            repeat.children.iter().map(|c| (c.label(), c.str("toggle-type"), c.int("toggle-state"))).collect();
        assert_eq!(
            marks,
            [("Off".into(), Some("radio".into()), Some(1)), ("All".into(), Some("radio".into()), Some(0)), ("One".into(), Some("radio".into()), Some(0)),]
        );
        let volume = root.child("Volume");
        assert_eq!(volume.str("children-display").as_deref(), Some("submenu"));
        assert_eq!(volume.outline(), ["This Computer", "—", "Volume Up", "Volume Down", "—", "0%", "25%", "50%", "75%", "100%"]);
        assert_eq!(volume.child("50%").int("toggle-state"), Some(1), "40% marks 50%");
        assert_eq!(volume.child("25%").int("toggle-state"), Some(0));
        assert!(!volume.child("This Computer").enabled());

        // Clicks, as the host sends them.
        let click = |id: i32| {
            let _: () = menu.call("Event", &(id, "clicked", Value::from(0i32), 0u32)).unwrap();
        };
        click(shuffle.id);
        assert_eq!(next("Shuffle"), Ev::Cmd(Cmd::Shuffle));
        click(repeat.child("One").id);
        assert_eq!(next("Repeat One"), Ev::Cmd(Cmd::Repeat(Repeat::One)));
        click(volume.child("Volume Up").id);
        assert_eq!(next("Volume Up"), Ev::Cmd(Cmd::VolumeSteps(1)));
        click(volume.child("100%").id);
        assert_eq!(next("100%"), Ev::Cmd(Cmd::Volume(100)));
        click(root.child("Quit").id);
        assert_eq!(next("Quit"), Ev::Cmd(Cmd::Quit));

        // The menu opening asks for fresh state.
        let _: bool = menu.call("AboutToShow", &(0i32,)).unwrap();
        assert_eq!(next("AboutToShow"), Ev::AboutToShow);

        // The app answers: Shuffle on, Repeat One, AirPlay at 75%. The host is told and reads the new marks.
        let mut updated = menu.receive_signal("ItemsPropertiesUpdated").unwrap();
        tray.set_modes(&Modes { shuffle: true, repeat: Repeat::One, autoplay: true, autoplay_offered: true });
        tray.set_output(&OutputVolume { output: Output::AirPlay, label: "Bedroom".into(), volume: Some(0.75) });
        assert!(updated.next().is_some(), "ItemsPropertiesUpdated");
        let root = wait_for("the new marks", || {
            let r = layout();
            (r.child("Shuffle").int("toggle-state") == Some(1) && r.outline().iter().any(|l| l == "Volume: 75%")).then_some(r)
        });
        assert_eq!(root.child("Repeat").child("One").int("toggle-state"), Some(1));
        assert_eq!(root.child("Repeat").child("Off").int("toggle-state"), Some(0));
        let volume = root.child("Volume: 75%");
        assert_eq!(volume.children[0].label(), "Bedroom");
        assert_eq!(volume.child("75%").int("toggle-state"), Some(1));
        assert_eq!(volume.child("50%").int("toggle-state"), Some(0));

        // Nothing loaded: the modes grey out and the song line goes.
        tray.set_song(None);
        let root = wait_for("the greyed modes", || {
            let r = layout();
            (!r.child("Shuffle").enabled()).then_some(r)
        });
        assert!(!root.child("Repeat").enabled() && !root.child("AutoPlay").enabled());
        assert_eq!(root.children[0].label(), "Pause");
        let tip: (String, Pixmaps, String, String) = item.get_property("ToolTip").unwrap();
        assert_eq!(tip.2, "Uzumaki");

        // Scrolling on the icon (KDE's scale: 120 a notch, up louder), then its end.
        let _: () = item.call("Scroll", &(120i32, "vertical")).unwrap();
        assert_eq!(next("scroll up"), Ev::Scroll(1));
        let _: () = item.call("Scroll", &(-240i32, "vertical")).unwrap();
        assert_eq!(next("scroll down"), Ev::Scroll(-2));
        let _: () = item.call("Scroll", &(120i32, "horizontal")).unwrap();
        let at = Instant::now();
        assert_eq!(next("the scroll's end"), Ev::ScrollEnd, "horizontal scrolls are ignored");
        assert!(at.elapsed() >= SCROLL_END - Duration::from_millis(150), "{:?}", at.elapsed());

        // A left click.
        let _: () = item.call("Activate", &(0i32, 0i32)).unwrap();
        assert_eq!(next("Activate"), Ev::Activate(None));

        // The icon changes in place.
        let mut new_icon = item.receive_signal("NewIcon").unwrap();
        tray.set_app_icon(IconVariant::Light);
        assert!(new_icon.next().is_some(), "NewIcon");
        let icons: Pixmaps = item.get_property("IconPixmap").unwrap();
        assert_eq!(icons[0].2, pixmaps(IconVariant::Light)[0].data);

        // Dropping it takes the item off the bus.
        drop(tray);
        let dbus = zbus::blocking::fdo::DBusProxy::new(&host).unwrap();
        wait_for("the item to leave the bus", || (!dbus.name_has_owner(name.as_str().try_into().unwrap()).unwrap()).then_some(()));
    }
}
