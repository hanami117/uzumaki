//! The current song's cover in the player, left of the song lines: kitty, sixel or iTerm2
//! graphics through `ratatui-image` when the terminal has them, else halfblocks (`▀` with two
//! colours). `UZUMAKI_COVER` picks how (`auto`, `off`, `halfblocks`, `kitty`, `sixel`,
//! `iterm2`); `i` shows or hides it for the session.
//!
//! The app names the cover's file in `Status::cover`. A worker reads, shrinks and encodes it once
//! per cover (`build`), keyed by the file and the terminal's cell size; drawing then costs
//! nothing, since ratatui only rewrites cells that changed. kitty images get our own ids, so the
//! one replaced or hidden can be deleted from the terminal (`take_deletes`).

use std::hash::{BuildHasher, Hasher};

use image::imageops::FilterType;
use ratatui::layout::Size;
use ratatui_image::picker::ProtocolType;
use ratatui_image::protocol::Protocol;
use ratatui_image::protocol::halfblocks::Halfblocks;
use ratatui_image::protocol::iterm2::Iterm2;
use ratatui_image::protocol::kitty::Kitty;
use ratatui_image::protocol::sixel::Sixel;
use ratatui_image::{FontSize, Resize};

use super::jobs::{Job, Jobs};
use super::probe::{self, Asked, Hints};

/// The cover is as tall as the player's rows.
pub const ROWS: u16 = 6;
/// Its width in columns stays within this, whatever the font's shape.
const MIN_COLS: u16 = 8;
const MAX_COLS: u16 = 16;
/// Columns between the cover and the song lines.
pub const GAP: u16 = 2;
/// The song lines keep at least this many columns, or the cover isn't shown.
pub const MIN_TEXT: usize = 24;
/// A cell size to assume when the terminal doesn't say (halfblocks only need the 1:2 shape).
const FALLBACK_FONT: (u16, u16) = (10, 20);
/// The big covers (showcase and solo views) stay within this many rows and columns: kitty's
/// placeholders can address 297, and more would only make huge escapes.
const MAX_BIG: u16 = 200;
/// Encoded sizes kept for the playing song (one per view), so cycling views re-encodes nothing.
const KEEP: usize = 3;

/// `UZUMAKI_COVER`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Ask the terminal what it can draw.
    Auto,
    /// Start hidden; `i` shows it (asking the terminal then).
    Off,
    /// Use this protocol whatever the terminal says.
    Force(ProtocolType),
}

impl Mode {
    pub fn parse(value: Option<&str>) -> Mode {
        match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("off" | "no" | "none" | "0" | "false") => Mode::Off,
            Some("halfblocks" | "blocks") => Mode::Force(ProtocolType::Halfblocks),
            Some("kitty") => Mode::Force(ProtocolType::Kitty),
            Some("sixel") => Mode::Force(ProtocolType::Sixel),
            Some("iterm2" | "iterm") => Mode::Force(ProtocolType::Iterm2),
            _ => Mode::Auto,
        }
    }

    pub fn from_env() -> Mode {
        Mode::parse(std::env::var("UZUMAKI_COVER").ok().or_else(|| std::env::var("CADENCE_COVER").ok()).as_deref())
    }
}

/// What the terminal can draw, and its cell size in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caps {
    pub proto: ProtocolType,
    /// Cell width and height in pixels.
    pub font: (u16, u16),
    /// Inside tmux: escapes go through its passthrough.
    pub tmux: bool,
}

impl Caps {
    #[cfg(test)]
    pub fn halfblocks(font: (u16, u16)) -> Caps {
        Caps { proto: ProtocolType::Halfblocks, font, tmux: false }
    }

    /// The cover's size in cells: `ROWS` tall, and as wide as makes it square on screen.
    pub fn cells(&self) -> Size {
        let (w, h) = (f32::from(self.font.0.max(1)), f32::from(self.font.1.max(1)));
        let cols = (f32::from(ROWS) * h / w).round() as u16;
        Size::new(cols.clamp(MIN_COLS, MAX_COLS), ROWS)
    }

    /// The biggest cover, square on screen, within `cols` × `rows` cells (the big views).
    /// Zero rows when not even one fits.
    pub fn square_in(&self, cols: u16, rows: u16) -> Size {
        let (w, h) = (f32::from(self.font.0.max(1)), f32::from(self.font.1.max(1)));
        (1..=rows.min(MAX_BIG))
            .rev()
            .map(|r| Size::new((f32::from(r) * h / w).round() as u16, r))
            .find(|s| s.width >= 1 && s.width <= cols.min(MAX_BIG))
            .unwrap_or(Size::new(0, 0))
    }
}

/// What [`detect`] found, and how (`uzu cover-info` prints it).
pub struct Detection {
    pub caps: Caps,
    /// Why this protocol.
    pub reason: String,
    /// The terminal query; `None` when halfblocks are forced (the terminal isn't asked).
    pub asked: Option<Asked>,
    /// Where the cell size came from (`None`: unknown, a guess is used).
    pub cell_source: Option<&'static str>,
    pub hints: Hints,
    /// Columns, rows, and the window's width and height in pixels (`TIOCGWINSZ`; 0 when unknown).
    pub window: Option<(u16, u16, u16, u16)>,
}

/// Asks the terminal what it can draw (`probe`), unless halfblocks are forced, and picks the
/// protocol and the cell size. Call it with the terminal in raw mode and before reading keys:
/// it reads the answer from the tty. Inside tmux the pane gets `allow-passthrough on`.
pub fn detect(mode: Mode) -> Detection {
    let hints = Hints::from_env();
    let forced = match mode {
        Mode::Force(p) => Some(p),
        _ => None,
    };
    let window = ratatui::crossterm::terminal::window_size().ok().map(|w| (w.columns, w.rows, w.width, w.height));
    let asked = (forced != Some(ProtocolType::Halfblocks)).then(|| probe::ask(hints.tmux, probe::TIMEOUT));
    let reply = asked.as_ref().map(|a| a.reply.clone()).unwrap_or_default();
    let (mut proto, mut reason) = probe::choose(&reply, &hints, forced);
    let cell = probe::cell_size(&reply, window.map(|(c, r, _, _)| (c, r)), font_from_window());
    // Pictures are sized from the cell size; without it they would come out the wrong size.
    if proto != ProtocolType::Halfblocks && cell.is_none() && forced.is_none() {
        reason = format!("{reason}; but no cell size, so halfblocks");
        proto = ProtocolType::Halfblocks;
    }
    let (font, cell_source) = match cell {
        Some((font, source)) => (font, Some(source)),
        None => (FALLBACK_FONT, None),
    };
    Detection { caps: Caps { proto, font, tmux: hints.tmux }, reason, asked, cell_source, hints, window }
}

/// `uzu cover-info`: the detection, readable and pasteable. `switch` is `UZUMAKI_COVER`.
pub fn report(d: &Detection, switch: Option<&str>) -> String {
    let h = &d.hints;
    let or_unset = |s: &str| if s.is_empty() { "(not set)".to_string() } else { s.to_string() };
    let version = std::env::var("TERM_PROGRAM_VERSION").unwrap_or_default();
    let mut lines = vec![
        "uzu cover-info".to_string(),
        format!("TERM_PROGRAM   {} {version}", or_unset(&h.term_program)).trim_end().to_string(),
        format!("TERM           {}", or_unset(&h.term)),
        format!("LC_TERMINAL    {}", or_unset(&h.lc_terminal)),
        format!("TMUX           {}", if h.tmux { "yes (this pane now allows passthrough)" } else { "no" }),
        format!("UZUMAKI_COVER  {}", switch.filter(|s| !s.is_empty()).unwrap_or("(not set: auto)")),
    ];
    let yes = |b: bool| if b { "yes" } else { "no" };
    let px = |s: Option<(u16, u16)>| s.map_or("none".to_string(), |(w, h)| format!("{w}x{h} px"));
    match &d.asked {
        None => lines.push("query          not sent (halfblocks forced)".into()),
        Some(a) => {
            let ms = a.elapsed.as_secs_f64() * 1000.0;
            let how = match (&a.error, a.timed_out) {
                (Some(e), _) => format!("failed after {ms:.1} ms: {e}"),
                (None, true) => format!("timed out after {ms:.0} ms"),
                (None, false) => format!("answered in {ms:.1} ms"),
            };
            let r = &a.reply;
            lines.push(format!("query          {how}"));
            lines.push(format!("replies        kitty {}, sixel {}, cell {}, text area {}, status {}", yes(r.kitty), yes(r.sixel), px(r.cell), px(r.area), yes(r.status)));
            let mut raw = probe::escaped(&a.raw);
            if raw.len() > 600 {
                raw.truncate(600);
                raw.push('…');
            }
            lines.push(format!("raw reply      {}", if raw.is_empty() { "(nothing)".into() } else { raw }));
        }
    }
    lines.push(match d.window {
        Some((c, r, w, hh)) => format!("window         {c}x{r} cells, {w}x{hh} px"),
        None => "window         unknown".into(),
    });
    let proto = match d.caps.proto {
        ProtocolType::Halfblocks => "halfblocks",
        ProtocolType::Kitty => "kitty",
        ProtocolType::Sixel => "sixel",
        ProtocolType::Iterm2 => "iterm2",
    };
    lines.push(format!("picked         {proto}: {}", d.reason));
    let (fw, fh) = d.caps.font;
    lines.push(format!("cell size      {fw}x{fh} px ({})", d.cell_source.unwrap_or("unknown, a guess")));
    let cells = d.caps.cells();
    lines.push(format!("cover          {}x{} cells, {}x{} px", cells.width, cells.height, u32::from(cells.width) * u32::from(fw), u32::from(cells.height) * u32::from(fh)));
    lines.join("\n")
}

/// The cell size from the terminal's pixel size (`TIOCGWINSZ` on Unix; ssh passes it on).
/// None when the terminal doesn't report pixels.
pub fn font_from_window() -> Option<(u16, u16)> {
    let ws = ratatui::crossterm::terminal::window_size().ok()?;
    if ws.width == 0 || ws.height == 0 || ws.columns == 0 || ws.rows == 0 {
        return None;
    }
    Some((ws.width / ws.columns, ws.height / ws.rows)).filter(|&(w, h)| w > 0 && h > 0)
}

/// Reads `path` and encodes it for `caps`, `area` cells at most (a square cover fills it).
/// Runs on a worker: decoding the 1000 px cover takes a few milliseconds.
pub fn build(path: &str, caps: Caps, area: Size, kitty_id: u32) -> Result<Protocol, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let img = image::load_from_memory(&bytes).map_err(|e| e.to_string())?;
    let font = FontSize::new(caps.font.0.max(1), caps.font.1.max(1));
    let (pw, ph) = (u32::from(area.width) * u32::from(font.width), u32::from(area.height) * u32::from(font.height));
    // Shrink once, keeping the shape, to the pixels the cells hold.
    let img = img.resize(pw, ph, FilterType::Triangle);
    let fit = Resize::Fit(None);
    let size = fit.size_for(&img, font, area);
    let err = |e: ratatui_image::errors::Errors| e.to_string();
    Ok(match caps.proto {
        // Halfblocks resample to the cells themselves; padding would show as a dark edge.
        ProtocolType::Halfblocks => Protocol::Halfblocks(Halfblocks::new(img, size).map_err(err)?),
        proto => {
            // Pad to whole cells (transparent), as the graphics protocols place whole cells.
            let img = fit.resize(&img, font, size, None);
            match proto {
                ProtocolType::Kitty => Protocol::Kitty(Kitty::new(img, size, kitty_id, caps.tmux, false).map_err(err)?),
                ProtocolType::Sixel => Protocol::Sixel(Sixel::new(img, size, caps.tmux).map_err(err)?),
                _ => Protocol::ITerm2(Iterm2::new(img, size, caps.tmux).map_err(err)?),
            }
        }
    })
}

/// The escape that deletes kitty image `id` and frees its data (`q=2`: no reply).
pub fn kitty_delete(id: u32, tmux: bool) -> String {
    let seq = format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\");
    if tmux { format!("\x1bPtmux;{}\x1b\\", seq.replace('\x1b', "\x1b\x1b")) } else { seq }
}

/// One encoded cover: which file at which caps and size, and for which song.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    pub song: String,
    pub path: String,
    pub caps: Caps,
    /// The cells it is encoded for (the view's slot).
    pub cells: Size,
}

impl Key {
    /// The same picture (song and file), whatever the size.
    fn same_file(&self, other: &Key) -> bool {
        self.song == other.song && self.path == other.path
    }
}

struct Ready {
    key: Key,
    protocol: Protocol,
    kitty_id: Option<u32>,
}

/// The cover's state on the screen: shown or not, what the terminal can do, the encoded image.
pub struct Cover {
    mode: Mode,
    shown: bool,
    caps: Option<Caps>,
    /// The terminal's cell size the last time we looked (resizes compare against it).
    window_font: Option<(u16, u16)>,
    /// The playing song's encodings, oldest first, one per size (`KEEP` at most).
    ready: Vec<Ready>,
    pending: Option<Key>,
    failed: Option<Key>,
    deletes: Vec<u32>,
    next_id: u32,
}

impl Cover {
    pub fn new(mode: Mode) -> Cover {
        // kitty ids are terminal-wide: start somewhere random so two uzus don't collide. 24 bits,
        // so the id fits the placeholder's colour.
        let seed = std::collections::hash_map::RandomState::new().build_hasher().finish() as u32;
        Cover { mode, shown: mode != Mode::Off, caps: None, window_font: None, ready: Vec::new(), pending: None, failed: None, deletes: Vec::new(), next_id: seed & 0x00ff_ffff }
    }

    /// Hidden, and never asks the terminal until `i` (tests, and the default before `run`).
    pub fn hidden() -> Cover {
        Cover::new(Mode::Off)
    }

    /// Shown, with these caps (tests).
    #[cfg(test)]
    pub fn with_caps(caps: Caps) -> Cover {
        let mut c = Cover::new(Mode::Auto);
        c.caps = Some(caps);
        c
    }

    /// Shown, but the terminal hasn't been asked yet: the loop runs [`detect`] before drawing.
    pub fn needs_detect(&self) -> bool {
        self.shown && self.caps.is_none()
    }

    /// The mode to ask with (a session started with `off` asks as `auto`).
    pub fn detect_mode(&self) -> Mode {
        if self.mode == Mode::Off { Mode::Auto } else { self.mode }
    }

    pub fn set_caps(&mut self, caps: Caps) {
        self.caps = Some(caps);
        self.window_font = font_from_window();
    }

    /// After a resize: a new cell size (the font was zoomed) re-encodes the cover.
    pub fn window_changed(&mut self, font: Option<(u16, u16)>) {
        let Some(font) = font else { return };
        if self.window_font.is_some_and(|old| old != font) {
            if let Some(caps) = &mut self.caps {
                caps.font = font;
            }
        }
        self.window_font = Some(font);
    }

    /// `i`. Hiding drops the image (and deletes it from a kitty terminal).
    pub fn toggle(&mut self) -> bool {
        self.shown = !self.shown;
        if !self.shown {
            self.drop_ready();
            self.pending = None;
        }
        self.shown
    }

    /// The columns the cover takes in the player, when it is shown at all.
    pub fn cols(&self) -> Option<u16> {
        self.caps().map(|c| c.cells().width)
    }

    /// What the terminal draws with, when the cover is shown.
    pub fn caps(&self) -> Option<Caps> {
        self.caps.filter(|_| self.shown)
    }

    /// The encoded cover of `song` for a slot of `cells`: the one made for that size, else
    /// (while it is being made) the biggest one that fits.
    pub fn image(&self, song: &str, cells: Size) -> Option<&Protocol> {
        let mine = || self.ready.iter().filter(|r| r.key.song == song);
        mine().find(|r| r.key.cells == cells).or_else(|| {
            let fits = |r: &&Ready| r.key.cells.width <= cells.width && r.key.cells.height <= cells.height;
            mine().filter(fits).max_by_key(|r| r.key.cells.height)
        })
        .map(|r| &r.protocol)
    }

    /// How many encodings are kept (tests).
    #[cfg(test)]
    pub fn kept(&self) -> usize {
        self.ready.len()
    }

    /// Follows the status: the playing song, its cover file, and the cells the view gives it
    /// (`None`: the view shows no cover). A new file, size or caps is encoded on the worker;
    /// another song drops the old pictures at once.
    pub fn sync(&mut self, song: Option<&str>, path: Option<&str>, cells: Option<Size>, jobs: &mut dyn Jobs) {
        let Some(caps) = self.caps() else { return };
        let stale: Vec<usize> = (0..self.ready.len()).filter(|&i| Some(self.ready[i].key.song.as_str()) != song || self.ready[i].key.caps != caps).collect();
        for i in stale.into_iter().rev() {
            self.drop_at(i);
        }
        let (Some(song), Some(path), Some(cells)) = (song, path, cells.filter(|c| c.width > 0 && c.height > 0)) else {
            self.pending = None;
            return;
        };
        let key = Key { song: song.to_string(), path: path.to_string(), caps, cells };
        // A file that failed fails at every size.
        if self.ready.iter().any(|r| r.key == key) || self.pending.as_ref() == Some(&key) || self.failed.as_ref().is_some_and(|f| f.same_file(&key) && f.caps == caps) {
            return;
        }
        self.next_id = (self.next_id + 1) & 0x00ff_ffff;
        if self.next_id == 0 {
            self.next_id = 1;
        }
        self.pending = Some(key.clone());
        jobs.submit(Job::Cover { key, kitty_id: self.next_id });
    }

    /// The worker's result. Anything but the cover waited for is dropped.
    pub fn done(&mut self, key: Key, kitty_id: u32, result: Result<Protocol, String>) {
        if self.pending.as_ref() != Some(&key) {
            return;
        }
        self.pending = None;
        match result {
            Ok(protocol) => {
                // This size replaces its old encoding, and a newer file all of the old file's.
                let old: Vec<usize> = (0..self.ready.len()).filter(|&i| self.ready[i].key.cells == key.cells || !self.ready[i].key.same_file(&key)).collect();
                for i in old.into_iter().rev() {
                    self.drop_at(i);
                }
                while self.ready.len() >= KEEP {
                    self.drop_at(0);
                }
                let kitty_id = matches!(protocol, Protocol::Kitty(_)).then_some(kitty_id);
                self.ready.push(Ready { key, protocol, kitty_id });
            }
            Err(_) => self.failed = Some(key),
        }
    }

    /// Escapes to write before the next frame: kitty images replaced or hidden.
    pub fn take_deletes(&mut self) -> String {
        let tmux = self.caps.is_some_and(|c| c.tmux);
        std::mem::take(&mut self.deletes).into_iter().map(|id| kitty_delete(id, tmux)).collect()
    }

    /// On exit: forget the picture (its kitty image goes with the next `take_deletes`).
    pub fn clear(&mut self) {
        self.drop_ready();
        self.pending = None;
    }

    /// Drops every encoding (kitty images are deleted with the next `take_deletes`).
    fn drop_ready(&mut self) {
        while !self.ready.is_empty() {
            self.drop_at(0);
        }
    }

    fn drop_at(&mut self, i: usize) {
        if let Some(id) = self.ready.remove(i).kitty_id {
            self.deletes.push(id);
        }
    }
}

/// A small JPEG cover in the temp folder, for tests: red left half, blue right half. A new file
/// per call: tests run in parallel and many share a name (`sololy-120x34`), and rewriting a file
/// another test is decoding gave that test a cut-off picture (no cover drawn).
#[cfg(test)]
pub fn test_jpeg(name: &str) -> String {
    static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("uzu-cover-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let n = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = dir.join(format!("{name}-{n}.jpg"));
    let img = image::RgbImage::from_fn(96, 96, |x, _| if x < 48 { image::Rgb([220, 30, 30]) } else { image::Rgb([30, 30, 220]) });
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 90).encode_image(&img).unwrap();
    std::fs::write(&path, bytes).unwrap();
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_comes_from_the_switch() {
        assert_eq!(Mode::parse(None), Mode::Auto);
        assert_eq!(Mode::parse(Some("auto")), Mode::Auto);
        assert_eq!(Mode::parse(Some("whatever")), Mode::Auto);
        assert_eq!(Mode::parse(Some(" OFF ")), Mode::Off);
        assert_eq!(Mode::parse(Some("0")), Mode::Off);
        assert_eq!(Mode::parse(Some("halfblocks")), Mode::Force(ProtocolType::Halfblocks));
        assert_eq!(Mode::parse(Some("kitty")), Mode::Force(ProtocolType::Kitty));
        assert_eq!(Mode::parse(Some("sixel")), Mode::Force(ProtocolType::Sixel));
        assert_eq!(Mode::parse(Some("iTerm2")), Mode::Force(ProtocolType::Iterm2));
        // Off starts hidden and asks as auto once `i` shows it.
        let mut c = Cover::new(Mode::Off);
        assert!(!c.needs_detect() && c.cols().is_none());
        assert!(c.toggle());
        assert!(c.needs_detect());
        assert_eq!(c.detect_mode(), Mode::Auto);
        assert_eq!(Cover::new(Mode::Force(ProtocolType::Sixel)).detect_mode(), Mode::Force(ProtocolType::Sixel));
    }

    #[test]
    fn cells_make_a_square_on_screen() {
        assert_eq!(Caps::halfblocks((10, 20)).cells(), Size::new(12, 6));
        assert_eq!(Caps::halfblocks((18, 38)).cells(), Size::new(13, 6), "Ghostty on a 2x screen");
        assert_eq!(Caps::halfblocks((20, 20)).cells(), Size::new(MIN_COLS, 6));
        assert_eq!(Caps::halfblocks((4, 40)).cells(), Size::new(MAX_COLS, 6));
        assert_eq!(Caps::halfblocks((0, 0)).cells().height, ROWS, "no division by zero");
    }

    #[test]
    fn square_in_is_the_biggest_square_that_fits() {
        let c = Caps::halfblocks((10, 20));
        // Height-bound: 20 rows are 40 columns.
        assert_eq!(c.square_in(100, 20), Size::new(40, 20));
        // Width-bound: 30 columns hold 15 rows.
        assert_eq!(c.square_in(30, 40), Size::new(30, 15));
        let ghostty = Caps::halfblocks((18, 38));
        let s = ghostty.square_in(200, 30);
        assert_eq!(s.height, 30);
        assert!((u32::from(s.width) * 18).abs_diff(30 * 38) <= 9, "square in pixels: {s:?}");
        assert_eq!(c.square_in(1, 0), Size::new(0, 0));
        assert_eq!(c.square_in(0, 10), Size::new(0, 0));
        assert!(c.square_in(1000, 1000).height <= MAX_BIG && c.square_in(1000, 1000).width <= MAX_BIG);
    }

    #[test]
    fn big_sizes_encode_at_their_cells() {
        let path = test_jpeg("big-size");
        for proto in [ProtocolType::Halfblocks, ProtocolType::Kitty, ProtocolType::Sixel, ProtocolType::Iterm2] {
            let caps = Caps { proto, font: (10, 20), tmux: false };
            let area = caps.square_in(80, 20);
            let p = build(&path, caps, area, 7).unwrap();
            assert_eq!(p.size(), area, "{proto:?}");
        }
    }

    #[test]
    fn kitty_delete_escapes() {
        assert_eq!(kitty_delete(42, false), "\x1b_Ga=d,d=I,i=42,q=2\x1b\\");
        assert_eq!(kitty_delete(42, true), "\x1bPtmux;\x1b\x1b_Ga=d,d=I,i=42,q=2\x1b\x1b\\\x1b\\");
    }

    #[test]
    fn build_encodes_each_protocol_at_the_cover_size() {
        let path = test_jpeg("build");
        for proto in [ProtocolType::Halfblocks, ProtocolType::Kitty, ProtocolType::Sixel, ProtocolType::Iterm2] {
            let caps = Caps { proto, font: (10, 20), tmux: false };
            let p = build(&path, caps, caps.cells(), 7).unwrap_or_else(|e| panic!("{proto:?}: {e}"));
            assert_eq!(p.size(), Size::new(12, 6), "{proto:?}");
            let kind_ok = match proto {
                ProtocolType::Halfblocks => matches!(p, Protocol::Halfblocks(_)),
                ProtocolType::Kitty => matches!(p, Protocol::Kitty(_)),
                ProtocolType::Sixel => matches!(p, Protocol::Sixel(_)),
                ProtocolType::Iterm2 => matches!(p, Protocol::ITerm2(_)),
            };
            assert!(kind_ok, "{proto:?}");
        }
        let caps = Caps::halfblocks((10, 20));
        assert!(build("/nonexistent/uzu-cover.jpg", caps, caps.cells(), 1).is_err());
        let junk = std::env::temp_dir().join(format!("uzu-cover-junk-{}", std::process::id()));
        std::fs::write(&junk, b"not a picture").unwrap();
        assert!(build(&junk.to_string_lossy(), caps, caps.cells(), 1).is_err());
        let _ = std::fs::remove_file(junk);
    }

    #[test]
    fn a_zoomed_font_changes_the_caps_only_after_a_change() {
        let mut c = Cover::with_caps(Caps { proto: ProtocolType::Kitty, font: (18, 38), tmux: false });
        c.window_font = None;
        // The first look only records (the queried size is more exact than the window's).
        c.window_changed(Some((18, 39)));
        assert_eq!(c.caps.unwrap().font, (18, 38));
        c.window_changed(Some((18, 39)));
        assert_eq!(c.caps.unwrap().font, (18, 38));
        // Then a zoom: the cells grew.
        c.window_changed(Some((22, 46)));
        assert_eq!(c.caps.unwrap().font, (22, 46));
        c.window_changed(None);
        assert_eq!(c.caps.unwrap().font, (22, 46));
    }
}
