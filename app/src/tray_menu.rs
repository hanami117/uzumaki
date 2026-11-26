//! The tray menu of Linux and Windows, as plain data: what `tray_linux.rs` (StatusNotifierItem +
//! dbusmenu, through ksni) and `tray_win.rs` (a Win32 popup menu) draw, and what their items do.
//! The same layout as the macOS menu bar menu (`tray_mac.rs`):
//!
//! ```text
//! Title — Artist            (greyed, only while a song is loaded)
//! ───
//! Play / Pause, Previous, Next
//! ───
//! ✓ Shuffle
//!   Repeat ▸ (•) Off  ( ) All  ( ) One
//! ✓ AutoPlay
//! ───
//!   Volume: 40% ▸  This Computer (greyed) │ Volume Up │ Volume Down │ (•) 0% … 100%
//! ───
//! Show Uzumaki, Check for Updates…
//! ───
//! Quit Uzumaki
//! ```
//!
//! No state of its own: `Snapshot` is what the tray was last told (`TrayCtl`'s setters, from the
//! UI thread), and `entries` is a pure function of it. Each `Cmd` calls what the window's controls
//! call: the modes as in `state/tray_modes.rs`, the volume through `State::set_output_volume`
//! (the output in use: the app, an AirPlay speaker group's master volume, a Chromecast).
//!
//! macOS has its own menu (a slider in place of the Volume submenu) and doesn't build this one;
//! the tests run everywhere.

use crate::state::output_volume::OutputVolume;
use crate::state::tray_modes::{repeat_word, Modes};
use crate::state::{Handle, Repeat, State};

/// What the menu shows. `Default` is the menu before the first sync: nothing loaded, all greyed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub playing: bool,
    /// "Title — Artist", or empty with no song.
    pub song: String,
    /// A song is loaded (the window's Shuffle, Repeat and ∞ buttons exist).
    pub loaded: bool,
    /// None before the first sync.
    pub modes: Option<Modes>,
    /// Where the sound goes and its volume; None before the first sync.
    pub output: Option<OutputVolume>,
}

impl Snapshot {
    /// The tooltip: the song, or the app's name.
    pub fn tooltip(&self) -> &str {
        if self.song.is_empty() { "Uzumaki" } else { &self.song }
    }

    /// The volume in whole percents, when the output has said it.
    pub fn percent(&self) -> Option<u8> {
        self.output.as_ref().and_then(|o| o.volume).map(|v| (v.clamp(0.0, 1.0) * 100.0).round() as u8)
    }
}

/// What a menu item (or a scroll on the icon) asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    PlayPause,
    Previous,
    Next,
    Shuffle,
    Repeat(Repeat),
    AutoPlay,
    /// `TRAY_STEP` percents louder (positive) or quieter, per step.
    VolumeSteps(i32),
    /// A volume in percent (the submenu's 0 … 100 items).
    Volume(u8),
    Show,
    CheckUpdates,
    Quit,
}

/// One menu entry.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    /// Greyed text (the song, the output's name).
    Label(String),
    Action { label: String, cmd: Cmd, enabled: bool },
    /// A toggle with a checkmark.
    Check { label: String, cmd: Cmd, checked: bool, enabled: bool },
    /// Radio items, one of which (or none) is marked.
    Radio { options: Vec<(String, Cmd)>, selected: Option<usize>, enabled: bool },
    Submenu { label: String, enabled: bool, items: Vec<Entry> },
    Separator,
}

/// The volume steps of the Volume submenu, in percent; the nearest one is marked.
pub const VOLUME_STEPS: [u8; 5] = [0, 25, 50, 75, 100];
/// What Volume Up / Down and one scroll notch change, in percent (as the keyboard's Volume keys).
pub const TRAY_STEP: i32 = 5;

const REPEATS: [Repeat; 3] = [Repeat::Off, Repeat::All, Repeat::One];

fn action(label: &str, cmd: Cmd) -> Entry {
    Entry::Action { label: label.into(), cmd, enabled: true }
}

/// The whole menu for `s`.
pub fn entries(s: &Snapshot) -> Vec<Entry> {
    let mut out = Vec::new();
    if !s.song.is_empty() {
        out.push(Entry::Label(s.song.clone()));
        out.push(Entry::Separator);
    }
    out.push(action(if s.playing { "Pause" } else { "Play" }, Cmd::PlayPause));
    out.push(action("Previous", Cmd::Previous));
    out.push(action("Next", Cmd::Next));
    out.push(Entry::Separator);
    let m = s.modes.unwrap_or(Modes { shuffle: false, repeat: Repeat::Off, autoplay: false, autoplay_offered: false });
    let synced = s.modes.is_some();
    let loaded = s.loaded && synced;
    out.push(Entry::Check { label: "Shuffle".into(), cmd: Cmd::Shuffle, checked: m.shuffle, enabled: loaded });
    out.push(Entry::Submenu {
        label: "Repeat".into(),
        enabled: loaded,
        items: vec![Entry::Radio {
            options: REPEATS.iter().map(|&r| (repeat_word(r).to_string(), Cmd::Repeat(r))).collect(),
            selected: REPEATS.iter().position(|&r| r == m.repeat),
            enabled: loaded,
        }],
    });
    out.push(Entry::Check { label: "AutoPlay".into(), cmd: Cmd::AutoPlay, checked: m.autoplay, enabled: synced && m.autoplay_enabled(s.loaded) });
    out.push(Entry::Separator);
    out.push(volume_menu(s));
    out.push(Entry::Separator);
    out.push(action("Show Uzumaki", Cmd::Show));
    out.push(action("Check for Updates\u{2026}", Cmd::CheckUpdates));
    out.push(Entry::Separator);
    out.push(action("Quit Uzumaki", Cmd::Quit));
    out
}

/// "Volume: 40%" ▸ where the sound goes, Volume Up / Down, and the steps with the nearest marked.
/// Greyed while the output hasn't said its volume (a Chromecast connecting) or before the first sync.
fn volume_menu(s: &Snapshot) -> Entry {
    let percent = s.percent();
    let known = percent.is_some();
    let label = match percent {
        Some(p) => format!("Volume: {p}%"),
        None => "Volume".into(),
    };
    let mut items = Vec::new();
    if let Some(o) = &s.output {
        items.push(Entry::Label(o.label.clone()));
        items.push(Entry::Separator);
    }
    items.push(Entry::Action { label: "Volume Up".into(), cmd: Cmd::VolumeSteps(1), enabled: known && percent < Some(100) });
    items.push(Entry::Action { label: "Volume Down".into(), cmd: Cmd::VolumeSteps(-1), enabled: known && percent > Some(0) });
    items.push(Entry::Separator);
    items.push(Entry::Radio {
        options: VOLUME_STEPS.iter().map(|&p| (format!("{p}%"), Cmd::Volume(p))).collect(),
        selected: percent.map(nearest_step),
        enabled: known,
    });
    Entry::Submenu { label, enabled: known, items }
}

/// The index in `VOLUME_STEPS` nearest to `percent`.
pub fn nearest_step(percent: u8) -> usize {
    let mut best = 0;
    for (i, &p) in VOLUME_STEPS.iter().enumerate() {
        if p.abs_diff(percent) < VOLUME_STEPS[best].abs_diff(percent) {
            best = i;
        }
    }
    best
}

/// Carries out `cmd`, from the tray's thread (each change runs on the UI thread). The tray is told
/// the new state afterwards, so its checkmarks and volume follow at once.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn run(handle: &Handle, cmd: Cmd) {
    match cmd {
        Cmd::Show => crate::tray::show_window(handle),
        // The answer shows in the window (the notice, or a toast).
        Cmd::CheckUpdates => {
            crate::tray::show_window(handle);
            handle.update(|s: &mut State, _| s.check_updates_now());
        }
        Cmd::Quit => handle.quit(),
        cmd => handle.update(move |s: &mut State, _| apply(s, cmd)),
    }
}

/// The state changes of `run` (everything but Show, Check for Updates and Quit).
pub fn apply(s: &mut State, cmd: Cmd) {
    match cmd {
        Cmd::PlayPause => s.toggle_play(),
        Cmd::Previous => s.previous(),
        Cmd::Next => s.next(),
        Cmd::Shuffle => s.toggle_shuffle(),
        Cmd::Repeat(r) => s.set_repeat(r),
        Cmd::AutoPlay => s.toggle_autoplay(),
        Cmd::VolumeSteps(n) => s.step_output_volume(n * TRAY_STEP, true),
        Cmd::Volume(p) => s.set_output_volume(f64::from(p) / 100.0, true),
        Cmd::Show | Cmd::CheckUpdates | Cmd::Quit => {}
    }
    s.sync_tray_modes();
}

/// The menu with a number per item, for menus that name items by number (Win32's command ids).
/// Ids start at 1 (0 is "nothing picked"); `cmds[id - 1]` is what item `id` does.
#[derive(Clone, Debug, PartialEq)]
pub enum Numbered {
    /// `id` 0 for a greyed label. `radio`: drawn with a dot instead of a checkmark.
    Item { id: usize, label: String, checked: bool, radio: bool, enabled: bool },
    Popup { label: String, enabled: bool, items: Vec<Numbered> },
    Separator,
}

#[cfg_attr(not(windows), allow(dead_code))]
pub fn numbered(entries: &[Entry]) -> (Vec<Numbered>, Vec<Cmd>) {
    fn walk(entries: &[Entry], cmds: &mut Vec<Cmd>) -> Vec<Numbered> {
        let mut out = Vec::new();
        let add = |cmds: &mut Vec<Cmd>, cmd: Cmd| {
            cmds.push(cmd);
            cmds.len()
        };
        for e in entries {
            match e {
                Entry::Label(label) => out.push(Numbered::Item { id: 0, label: label.clone(), checked: false, radio: false, enabled: false }),
                Entry::Action { label, cmd, enabled } => {
                    out.push(Numbered::Item { id: add(cmds, *cmd), label: label.clone(), checked: false, radio: false, enabled: *enabled })
                }
                Entry::Check { label, cmd, checked, enabled } => {
                    out.push(Numbered::Item { id: add(cmds, *cmd), label: label.clone(), checked: *checked, radio: false, enabled: *enabled })
                }
                Entry::Radio { options, selected, enabled } => {
                    for (i, (label, cmd)) in options.iter().enumerate() {
                        let checked = *selected == Some(i);
                        out.push(Numbered::Item { id: add(cmds, *cmd), label: label.clone(), checked, radio: true, enabled: *enabled });
                    }
                }
                Entry::Submenu { label, enabled, items } => {
                    out.push(Numbered::Popup { label: label.clone(), enabled: *enabled, items: walk(items, cmds) })
                }
                Entry::Separator => out.push(Numbered::Separator),
            }
        }
        out
    }
    let mut cmds = Vec::new();
    let items = walk(entries, &mut cmds);
    (items, cmds)
}

/// Turns the `Scroll(delta, orientation)` calls of a StatusNotifierItem host into volume steps.
/// Hosts disagree on both scale and sign: KDE Plasma sends Qt's wheel angle (120 per notch, small
/// values from a touchpad, up positive); GTK hosts (GNOME's AppIndicator extension, XFCE,
/// waybar) send about ±1 per notch or touchpad event with GTK's sign (down positive), and don't
/// gather a touchpad's small moves, so their steps are spaced out by `GTK_GAP`.
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
#[derive(Debug)]
pub struct ScrollSteps {
    kde: bool,
    acc: i32,
    last: Option<std::time::Instant>,
}

/// A notch of Qt's wheel.
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
const QT_NOTCH: i32 = 120;
/// GTK hosts: at most one step per this (a touchpad sends an event every few ms).
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
const GTK_GAP: std::time::Duration = std::time::Duration::from_millis(80);

#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
impl ScrollSteps {
    /// `kde`: the host is KDE Plasma's (Qt's scale and sign).
    pub fn new(kde: bool) -> Self {
        Self { kde, acc: 0, last: None }
    }

    /// A vertical scroll: the steps it makes, louder positive.
    pub fn push(&mut self, delta: i32, now: std::time::Instant) -> i32 {
        if delta == 0 {
            return 0;
        }
        if self.kde {
            if self.acc.signum() * delta.signum() < 0 {
                self.acc = 0; // turning back answers at once
            }
            self.acc = self.acc.saturating_add(delta);
            let n = self.acc / QT_NOTCH;
            self.acc -= n * QT_NOTCH;
            return n;
        }
        if self.last.is_some_and(|t| now.saturating_duration_since(t) < GTK_GAP) {
            return 0;
        }
        self.last = Some(now);
        -delta.signum() * delta.abs().min(3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::output_volume::Output;

    fn snap() -> Snapshot {
        Snapshot {
            playing: true,
            song: "Song — Artist".into(),
            loaded: true,
            modes: Some(Modes { shuffle: true, repeat: Repeat::All, autoplay: false, autoplay_offered: true }),
            output: Some(OutputVolume { output: Output::AirPlay, label: "Bedroom".into(), volume: Some(0.4) }),
        }
    }

    fn find<'a>(items: &'a [Entry], name: &str) -> &'a Entry {
        items
            .iter()
            .find(|e| match e {
                Entry::Action { label, .. } | Entry::Check { label, .. } | Entry::Submenu { label, .. } => label.starts_with(name),
                Entry::Label(l) => l == name,
                _ => false,
            })
            .unwrap_or_else(|| panic!("no {name} in {items:?}"))
    }

    /// The labels, in order, with "—" for separators (the menu's outline).
    fn outline(items: &[Entry]) -> Vec<String> {
        items
            .iter()
            .map(|e| match e {
                Entry::Label(l) => format!("[{l}]"),
                Entry::Action { label, .. } | Entry::Check { label, .. } => label.clone(),
                Entry::Submenu { label, .. } => format!("{label} >"),
                Entry::Radio { options, .. } => options.iter().map(|o| o.0.as_str()).collect::<Vec<_>>().join("|"),
                Entry::Separator => "—".into(),
            })
            .collect()
    }

    #[test]
    fn the_menu_has_the_mac_menus_layout() {
        assert_eq!(
            outline(&entries(&snap())),
            [
                "[Song — Artist]", "—", "Pause", "Previous", "Next", "—", "Shuffle", "Repeat >", "AutoPlay", "—", "Volume: 40% >", "—",
                "Show Uzumaki", "Check for Updates\u{2026}", "—", "Quit Uzumaki"
            ]
        );
        let idle = Snapshot { song: String::new(), loaded: false, playing: false, ..snap() };
        assert_eq!(outline(&entries(&idle))[..2], ["Play", "Previous"], "no song line with nothing loaded");
    }

    #[test]
    fn shuffle_and_autoplay_are_checkmarks_and_repeat_a_radio_submenu() {
        let items = entries(&snap());
        assert_eq!(find(&items, "Shuffle"), &Entry::Check { label: "Shuffle".into(), cmd: Cmd::Shuffle, checked: true, enabled: true });
        assert!(matches!(find(&items, "AutoPlay"), Entry::Check { checked: false, enabled: true, .. }));
        let Entry::Submenu { enabled: true, items: sub, .. } = find(&items, "Repeat") else { panic!() };
        assert_eq!(
            sub,
            &[Entry::Radio {
                options: vec![("Off".into(), Cmd::Repeat(Repeat::Off)), ("All".into(), Cmd::Repeat(Repeat::All)), ("One".into(), Cmd::Repeat(Repeat::One))],
                selected: Some(1),
                enabled: true,
            }]
        );
    }

    #[test]
    fn the_modes_are_greyed_where_the_window_has_no_buttons() {
        let idle = Snapshot { loaded: false, ..snap() };
        let items = entries(&idle);
        assert!(matches!(find(&items, "Shuffle"), Entry::Check { checked: true, enabled: false, .. }), "still shows its state");
        assert!(matches!(find(&items, "Repeat"), Entry::Submenu { enabled: false, .. }));
        assert!(matches!(find(&items, "AutoPlay"), Entry::Check { enabled: false, .. }));
        let mut local = snap();
        local.modes = local.modes.map(|m| Modes { autoplay_offered: false, ..m });
        let items = entries(&local);
        assert!(matches!(find(&items, "Shuffle"), Entry::Check { enabled: true, .. }));
        assert!(matches!(find(&items, "AutoPlay"), Entry::Check { enabled: false, .. }), "no ∞ button: greyed");
        let first = entries(&Snapshot { playing: true, ..Snapshot::default() });
        assert_eq!(outline(&first)[0], "Pause");
        for name in ["Shuffle", "Repeat", "AutoPlay", "Volume"] {
            assert!(
                matches!(find(&first, name), Entry::Check { enabled: false, .. } | Entry::Submenu { enabled: false, .. }),
                "{name} greyed before the first sync"
            );
        }
    }

    #[test]
    fn the_volume_submenu_names_the_output_and_marks_the_nearest_step() {
        let items = entries(&snap());
        let Entry::Submenu { label, enabled: true, items: sub } = find(&items, "Volume") else { panic!() };
        assert_eq!(label, "Volume: 40%");
        assert_eq!(outline(sub), ["[Bedroom]", "—", "Volume Up", "Volume Down", "—", "0%|25%|50%|75%|100%"]);
        let Entry::Radio { selected, enabled: true, options } = &sub[5] else { panic!() };
        assert_eq!(*selected, Some(2), "40% is nearest to 50%");
        assert_eq!(options[4].1, Cmd::Volume(100));
        assert!(matches!(find(sub, "Volume Up"), Entry::Action { cmd: Cmd::VolumeSteps(1), enabled: true, .. }));
        assert!(matches!(find(sub, "Volume Down"), Entry::Action { cmd: Cmd::VolumeSteps(-1), enabled: true, .. }));
        for (p, want) in [(0, 0), (12, 0), (13, 1), (37, 1), (38, 2), (62, 2), (63, 3), (87, 3), (88, 4), (100, 4)] {
            assert_eq!(nearest_step(p), want, "{p}%");
        }
    }

    #[test]
    fn the_volume_ends_and_unknown_volumes_grey_what_cannot_be_used() {
        let at = |v: Option<f64>| {
            let mut s = snap();
            s.output = Some(OutputVolume { output: Output::Cast, label: "Smart TV".into(), volume: v });
            entries(&s)
        };
        let full = at(Some(1.0));
        let Entry::Submenu { items: sub, .. } = find(&full, "Volume") else { panic!() };
        assert!(matches!(find(sub, "Volume Up"), Entry::Action { enabled: false, .. }));
        assert!(matches!(find(sub, "Volume Down"), Entry::Action { enabled: true, .. }));
        let connecting = at(None);
        let Entry::Submenu { label, enabled, items: sub } = find(&connecting, "Volume") else { panic!() };
        assert_eq!((label.as_str(), *enabled), ("Volume", false), "a Chromecast that hasn't said its volume");
        assert!(matches!(&sub[5], Entry::Radio { selected: None, enabled: false, .. }));
        assert_eq!(sub[0], Entry::Label("Smart TV".into()));
    }

    #[test]
    fn numbering_gives_each_item_its_command_and_marks_radio_items() {
        let (items, cmds) = numbered(&entries(&snap()));
        assert_eq!(items[0], Numbered::Item { id: 0, label: "Song — Artist".into(), checked: false, radio: false, enabled: false });
        assert_eq!(cmds[0], Cmd::PlayPause, "id 1");
        assert_eq!(*cmds.last().unwrap(), Cmd::Quit);
        let shuffle = items.iter().find(|i| matches!(i, Numbered::Item { label, .. } if label == "Shuffle")).unwrap();
        let Numbered::Item { id, checked: true, radio: false, enabled: true, .. } = shuffle else { panic!("{shuffle:?}") };
        assert_eq!(cmds[id - 1], Cmd::Shuffle);
        let Some(Numbered::Popup { items: repeat, enabled: true, .. }) = items.iter().find(|i| matches!(i, Numbered::Popup { label, .. } if label == "Repeat")) else {
            panic!()
        };
        let marks: Vec<(bool, bool, Cmd)> = repeat
            .iter()
            .map(|i| match i {
                Numbered::Item { id, checked, radio, .. } => (*checked, *radio, cmds[id - 1]),
                _ => panic!(),
            })
            .collect();
        assert_eq!(marks, [(false, true, Cmd::Repeat(Repeat::Off)), (true, true, Cmd::Repeat(Repeat::All)), (false, true, Cmd::Repeat(Repeat::One))]);
        let Some(Numbered::Popup { items: vol, .. }) = items.iter().find(|i| matches!(i, Numbered::Popup { label, .. } if label.starts_with("Volume"))) else {
            panic!()
        };
        assert!(matches!(&vol[0], Numbered::Item { id: 0, enabled: false, label, .. } if label == "Bedroom"));
        let checked: Vec<Cmd> = vol.iter().filter_map(|i| match i {
            Numbered::Item { id, checked: true, .. } => Some(cmds[id - 1]),
            _ => None,
        }).collect();
        assert_eq!(checked, [Cmd::Volume(50)], "one step marked");
        // Every command id is unique and in range.
        let mut seen = std::collections::HashSet::new();
        fn ids(items: &[Numbered], seen: &mut std::collections::HashSet<usize>, n: usize) {
            for i in items {
                match i {
                    Numbered::Item { id, .. } if *id > 0 => assert!(seen.insert(*id) && *id <= n, "{id}"),
                    Numbered::Popup { items: sub, .. } => ids(sub, seen, n),
                    _ => {}
                }
            }
        }
        ids(&items, &mut seen, cmds.len());
        assert_eq!(seen.len(), cmds.len());
    }

    #[test]
    fn the_tooltip_is_the_song_or_the_apps_name() {
        assert_eq!(snap().tooltip(), "Song — Artist");
        assert_eq!(Snapshot::default().tooltip(), "Uzumaki");
    }

    #[test]
    fn commands_do_what_the_windows_controls_do() {
        let mut s = State::offline();
        s.playback.volume = 0.4;
        apply(&mut s, Cmd::Shuffle);
        assert!(s.playback.shuffle);
        apply(&mut s, Cmd::Repeat(Repeat::One));
        assert_eq!(s.playback.repeat, Repeat::One);
        let on = s.settings.autoplay;
        apply(&mut s, Cmd::AutoPlay);
        assert_eq!(s.settings.autoplay, !on);
        apply(&mut s, Cmd::VolumeSteps(1));
        assert!((s.playback.volume - 0.45).abs() < 1e-9, "{}", s.playback.volume);
        apply(&mut s, Cmd::VolumeSteps(-2));
        assert!((s.playback.volume - 0.35).abs() < 1e-9);
        apply(&mut s, Cmd::Volume(75));
        assert_eq!(s.playback.volume, 0.75);
        apply(&mut s, Cmd::Volume(100));
        apply(&mut s, Cmd::VolumeSteps(3));
        assert_eq!(s.playback.volume, 1.0, "no further than full");
    }

    #[test]
    fn kde_scrolls_in_notches_of_120_up_louder() {
        let t = std::time::Instant::now();
        let mut k = ScrollSteps::new(true);
        assert_eq!(k.push(120, t), 1);
        assert_eq!(k.push(240, t), 2);
        assert_eq!(k.push(-120, t), -1);
        // A touchpad's small moves add up; turning back starts again.
        assert_eq!((0..7).map(|_| k.push(20, t)).sum::<i32>(), 1);
        assert_eq!(k.push(-20, t), 0);
        assert_eq!((0..5).map(|_| k.push(-20, t)).sum::<i32>(), -1);
        assert_eq!(k.push(0, t), 0);
    }

    #[test]
    fn gtk_hosts_scroll_one_step_per_event_down_quieter_spaced_out() {
        let t = std::time::Instant::now();
        let ms = |n: u64| t + std::time::Duration::from_millis(n);
        let mut g = ScrollSteps::new(false);
        assert_eq!(g.push(-1, ms(0)), 1, "up (negative in GTK) is louder");
        assert_eq!(g.push(-1, ms(10)), 0, "a touchpad's burst is spaced out");
        assert_eq!(g.push(1, ms(100)), -1);
        assert_eq!(g.push(7, ms(200)), -3, "a big accumulated move: a few steps, not seven");
    }
}
