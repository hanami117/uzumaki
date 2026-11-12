//! Asks the terminal what it can draw: kitty graphics, sixel, and its cell size in pixels.
//!
//! Our own query rather than `ratatui-image`'s: it runs on the calling thread and reads the tty
//! straight from file descriptor 0 (no reader thread left behind after a timeout, no stdout lock
//! to wait for), keeps the raw replies for `uzu cover-info`, and doesn't drop a kitty answer
//! when the cell size is missing. The terminal must be in raw mode.

use std::time::{Duration, Instant};

use ratatui_image::picker::ProtocolType;

/// How long to wait for the replies. Every terminal answers the final status request at once;
/// this only matters for one that answers nothing.
pub const TIMEOUT: Duration = Duration::from_secs(2);

/// What the environment says about the terminal (ssh doesn't pass most of it on).
#[derive(Clone, Debug, Default)]
pub struct Hints {
    pub term_program: String,
    pub term: String,
    pub lc_terminal: String,
    /// Inside tmux: the query and the images go through its passthrough.
    pub tmux: bool,
    pub zellij: bool,
    pub konsole: bool,
    pub wezterm: bool,
    pub kitty_window: bool,
}

impl Hints {
    pub fn from_env() -> Hints {
        let var = |k: &str| std::env::var(k).unwrap_or_default();
        let set = |k: &str| !var(k).is_empty();
        let (term_program, term) = (var("TERM_PROGRAM"), var("TERM"));
        Hints {
            tmux: set("TMUX") || term.starts_with("tmux") || term_program == "tmux",
            zellij: set("ZELLIJ"),
            konsole: set("KONSOLE_VERSION"),
            wezterm: set("WEZTERM_EXECUTABLE") || term_program == "WezTerm",
            kitty_window: set("KITTY_WINDOW_ID"),
            lc_terminal: var("LC_TERMINAL"),
            term_program,
            term,
        }
    }

    /// Terminals that show iTerm2's inline images (`ratatui-image`'s list).
    fn iterm2(&self) -> bool {
        const NAMES: [&str; 9] = ["iTerm", "WezTerm", "mintty", "vscode", "Tabby", "Hyper", "rio", "Bobcat", "WarpTerminal"];
        NAMES.iter().any(|n| self.term_program.contains(n)) || self.lc_terminal.contains("iTerm")
    }
}

/// What the terminal answered.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reply {
    /// `ESC _G i=31;OK ESC \`: kitty graphics.
    pub kitty: bool,
    /// Device attributes (`ESC [ ? … c`) list 4: sixel.
    pub sixel: bool,
    /// `ESC [ 6 ; h ; w t`: one cell, width × height in pixels.
    pub cell: Option<(u16, u16)>,
    /// `ESC [ 4 ; h ; w t`: the text area, width × height in pixels.
    pub area: Option<(u16, u16)>,
    /// `ESC [ 0 n`: the status report that ends the replies.
    pub status: bool,
}

/// The query: kitty graphics (a 1×1 test image, query only), cell size, text area size, device
/// attributes, then a status report every terminal answers, last. Inside tmux the whole of it
/// goes to the outer terminal through tmux's passthrough.
pub fn query_text(tmux: bool) -> String {
    let inner = "\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[16t\x1b[14t\x1b[c\x1b[5n";
    if tmux { format!("\x1bPtmux;{}\x1b\\", inner.replace('\x1b', "\x1b\x1b")) } else { inner.to_string() }
}

/// Reads the replies out of `bytes`; anything else (mouse reports, keys) is skipped.
pub fn parse(bytes: &[u8]) -> Reply {
    let mut r = Reply::default();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != 0x1b || i + 1 >= bytes.len() {
            i += 1;
            continue;
        }
        match bytes[i + 1] {
            // APC: `ESC _ … ESC \`
            b'_' => {
                let body_start = i + 2;
                let end = find(bytes, body_start, b"\x1b\\").unwrap_or(bytes.len());
                let body = &bytes[body_start..end];
                if body.starts_with(b"Gi=31") && find(body, 0, b";OK").is_some() {
                    r.kitty = true;
                }
                i = end + 2;
            }
            // CSI: `ESC [` parameters, then a final byte in 0x40..=0x7e.
            b'[' => {
                let start = i + 2;
                let Some(end) = (start..bytes.len()).find(|&j| bytes[j] == 0x1b || (0x40..=0x7e).contains(&bytes[j])) else { break };
                if bytes[end] == 0x1b {
                    i = end; // cut short by another sequence: start over there
                    continue;
                }
                let params = String::from_utf8_lossy(&bytes[start..end]);
                let nums = |p: &str| p.split(';').map(|n| n.parse::<u16>().ok()).collect::<Vec<_>>();
                match bytes[end] {
                    b'c' if params.starts_with('?') => r.sixel |= params[1..].split(';').any(|p| p == "4"),
                    b't' => match nums(&params)[..] {
                        [Some(6), Some(h), Some(w)] if w > 0 && h > 0 => r.cell = Some((w, h)),
                        [Some(4), Some(h), Some(w)] if w > 0 && h > 0 => r.area = Some((w, h)),
                        _ => {}
                    },
                    b'n' if params == "0" => r.status = true,
                    _ => {}
                }
                i = end + 1;
            }
            _ => i += 1,
        }
    }
    r
}

fn find(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    hay.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}

/// One query: the replies, how long they took, and what went wrong.
#[derive(Clone, Debug, Default)]
pub struct Asked {
    pub reply: Reply,
    pub raw: Vec<u8>,
    pub elapsed: Duration,
    /// No status report before the timeout.
    pub timed_out: bool,
    pub error: Option<String>,
}

/// Writes the query to the terminal and reads until the status report (or `timeout`).
/// The terminal must be in raw mode. Inside tmux, its pane first gets `allow-passthrough on`.
#[cfg(unix)]
pub fn ask(tmux: bool, timeout: Duration) -> Asked {
    use std::io::Write;
    if tmux {
        allow_tmux_passthrough();
    }
    let start = Instant::now();
    let mut asked = Asked::default();
    let mut out = std::io::stdout();
    if let Err(e) = out.write_all(query_text(tmux).as_bytes()).and_then(|()| out.flush()) {
        asked.error = Some(format!("writing the query: {e}"));
        return asked;
    }
    let mut buf = [0u8; 1024];
    loop {
        let left = timeout.saturating_sub(start.elapsed());
        if left.is_zero() {
            asked.timed_out = true;
            break;
        }
        let mut fds = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
        let ms = left.as_millis().clamp(1, i32::MAX as u128) as i32;
        // SAFETY: one valid pollfd, count 1.
        let n = unsafe { libc::poll(&mut fds, 1, ms) };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            asked.error = Some(format!("waiting for the reply: {e}"));
            break;
        }
        if n == 0 {
            continue; // the loop checks the time
        }
        // SAFETY: `buf` is writable for its length.
        let got = unsafe { libc::read(0, buf.as_mut_ptr().cast(), buf.len()) };
        if got <= 0 {
            asked.error = Some(if got == 0 { "the terminal closed its input".into() } else { format!("reading the reply: {}", std::io::Error::last_os_error()) });
            break;
        }
        asked.raw.extend_from_slice(&buf[..got as usize]);
        if parse(&asked.raw).status {
            break;
        }
    }
    asked.elapsed = start.elapsed();
    asked.reply = parse(&asked.raw);
    asked
}

#[cfg(not(unix))]
pub fn ask(_tmux: bool, _timeout: Duration) -> Asked {
    Asked { error: Some("not on this system yet".into()), ..Default::default() }
}

/// tmux drops escapes it doesn't know unless the pane allows passthrough (tmux 3.3+). This
/// changes the pane's option for good, as `ratatui-image` did.
#[cfg(unix)]
fn allow_tmux_passthrough() {
    use std::process::{Command, Stdio};
    let _ = Command::new("tmux")
        .args(["set", "-p", "allow-passthrough", "on"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Which protocol to draw with, and why. `forced` comes from `UZUMAKI_COVER`.
pub fn choose(reply: &Reply, hints: &Hints, forced: Option<ProtocolType>) -> (ProtocolType, String) {
    if let Some(p) = forced {
        return (p, "UZUMAKI_COVER".into());
    }
    if hints.konsole {
        return (ProtocolType::Halfblocks, "Konsole (its kitty and sixel images are unreliable)".into());
    }
    if hints.wezterm {
        return (ProtocolType::Iterm2, "WezTerm".into());
    }
    if reply.kitty {
        return (ProtocolType::Kitty, "the terminal answered the kitty graphics query".into());
    }
    if hints.iterm2() {
        return (ProtocolType::Iterm2, format!("TERM_PROGRAM={}", hints.term_program));
    }
    if reply.sixel {
        return (ProtocolType::Sixel, "the terminal lists sixel".into());
    }
    if !hints.tmux && !hints.zellij {
        // Rex is built on libghostty (its app bundles GhosttyVT's kitty graphics, virtual
        // placements included), so a missing kitty answer from it is a lost reply, not a no.
        if hints.term_program == "rex" {
            return (ProtocolType::Kitty, "TERM_PROGRAM=rex (built on libghostty), no kitty reply".into());
        }
        // Ghostty and kitty always answer; trust their names only when nothing came back.
        let kitty_family = hints.term_program == "ghostty" || hints.term == "xterm-ghostty" || hints.term == "xterm-kitty" || hints.kitty_window;
        if kitty_family && !reply.status {
            return (ProtocolType::Kitty, "a kitty-graphics terminal by its name; it didn't answer".into());
        }
    }
    let why = if reply.status { "no graphics in the terminal's replies" } else { "the terminal didn't answer" };
    (ProtocolType::Halfblocks, why.into())
}

/// The cell size in pixels and where it came from: the cell-size reply, else the text area
/// over the cells, else the window's pixel size (`TIOCGWINSZ`).
pub fn cell_size(reply: &Reply, cells: Option<(u16, u16)>, window: Option<(u16, u16)>) -> Option<((u16, u16), &'static str)> {
    if let Some(c) = reply.cell {
        return Some((c, "CSI 16 t"));
    }
    if let (Some((w, h)), Some((cols, rows))) = (reply.area, cells) {
        if cols > 0 && rows > 0 && w / cols > 0 && h / rows > 0 {
            return Some(((w / cols, h / rows), "CSI 14 t"));
        }
    }
    window.map(|w| (w, "window size"))
}

/// Bytes shown readably: `\e` for escape, `\xNN` for other control bytes.
pub fn escaped(bytes: &[u8]) -> String {
    let mut s = String::new();
    for &b in bytes {
        match b {
            0x1b => s.push_str("\\e"),
            b'\\' => s.push_str("\\\\"),
            0x20..=0x7e => s.push(b as char),
            _ => s.push_str(&format!("\\x{b:02x}")),
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const GHOSTTY: &[u8] = b"\x1b_Gi=31;OK\x1b\\\x1b[6;34;16t\x1b[4;1020;1920t\x1b[?62;22c\x1b[0n";

    #[test]
    fn reads_a_full_answer_and_skips_mouse_reports() {
        let r = parse(GHOSTTY);
        assert_eq!(r, Reply { kitty: true, sixel: false, cell: Some((16, 34)), area: Some((1920, 1020)), status: true });
        let mut noisy = b"\x1b[<35;10;5M".to_vec();
        noisy.extend_from_slice(&GHOSTTY[..12]);
        noisy.extend_from_slice(b"\x1b[<35;11;5Mq");
        noisy.extend_from_slice(&GHOSTTY[12..]);
        assert_eq!(parse(&noisy), parse(GHOSTTY));
        // A sequence cut short by another one doesn't swallow it.
        assert_eq!(parse(b"\x1b[6;3\x1b[6;34;16t").cell, Some((16, 34)));
    }

    #[test]
    fn reads_partial_and_negative_answers() {
        // A sixel terminal without kitty or cell size.
        let r = parse(b"\x1b[?62;4;22c\x1b[0n");
        assert_eq!(r, Reply { sixel: true, status: true, ..Default::default() });
        // kitty says no (an error instead of OK).
        assert!(!parse(b"\x1b_Gi=31;ENOTSUPPORTED:no\x1b\\\x1b[0n").kitty);
        // Cut short: nothing complete yet.
        assert_eq!(parse(b"\x1b_Gi=31;O"), Reply::default());
        assert_eq!(parse(b"\x1b[6;34"), Reply::default());
        // A zero size is no size.
        assert_eq!(parse(b"\x1b[6;0;0t").cell, None);
    }

    #[test]
    fn query_goes_through_tmux_whole() {
        assert!(query_text(false).starts_with("\x1b_Gi=31,") && query_text(false).ends_with("\x1b[5n"));
        let t = query_text(true);
        assert!(t.starts_with("\x1bPtmux;\x1b\x1b_Gi=31,") && t.ends_with("\x1b\x1b[5n\x1b\\"), "{t:?}");
    }

    fn hints(term_program: &str, term: &str) -> Hints {
        Hints { term_program: term_program.into(), term: term.into(), ..Default::default() }
    }

    #[test]
    fn chooses_what_answered_then_names() {
        let ghostty = parse(GHOSTTY);
        let none = Reply { status: true, ..Default::default() };
        let silent = Reply::default();
        let pick = |r: &Reply, h: &Hints| choose(r, h, None).0;
        assert_eq!(pick(&ghostty, &hints("ghostty", "xterm-ghostty")), ProtocolType::Kitty);
        assert_eq!(pick(&ghostty, &hints("anything", "xterm")), ProtocolType::Kitty);
        assert_eq!(pick(&none, &hints("iTerm.app", "xterm-256color")), ProtocolType::Iterm2);
        assert_eq!(pick(&Reply { sixel: true, ..none.clone() }, &hints("foot", "foot")), ProtocolType::Sixel);
        assert_eq!(pick(&none, &hints("Apple_Terminal", "xterm-256color")), ProtocolType::Halfblocks);
        // Rex: libghostty, so kitty even without the reply; Ghostty only when nothing answered.
        assert_eq!(pick(&none, &hints("rex", "xterm-256color")), ProtocolType::Kitty);
        assert_eq!(pick(&none, &hints("ghostty", "xterm-ghostty")), ProtocolType::Halfblocks);
        assert_eq!(pick(&silent, &hints("ghostty", "xterm-ghostty")), ProtocolType::Kitty);
        // Not through a multiplexer that hides the outer terminal.
        let mut in_zellij = hints("rex", "xterm-256color");
        in_zellij.zellij = true;
        assert_eq!(pick(&none, &in_zellij), ProtocolType::Halfblocks);
        // Konsole and WezTerm by name, whatever they answer.
        let mut konsole = hints("", "xterm-256color");
        konsole.konsole = true;
        assert_eq!(pick(&ghostty, &konsole), ProtocolType::Halfblocks);
        let mut wezterm = hints("WezTerm", "xterm-256color");
        wezterm.wezterm = true;
        assert_eq!(pick(&ghostty, &wezterm), ProtocolType::Iterm2);
        // The switch wins.
        assert_eq!(choose(&none, &hints("", ""), Some(ProtocolType::Sixel)).0, ProtocolType::Sixel);
    }

    #[test]
    fn cell_size_falls_back_in_order() {
        let both = parse(GHOSTTY);
        assert_eq!(cell_size(&both, Some((120, 30)), Some((15, 33))), Some(((16, 34), "CSI 16 t")));
        let area_only = Reply { area: Some((1920, 1020)), ..Default::default() };
        assert_eq!(cell_size(&area_only, Some((120, 30)), Some((15, 33))), Some(((16, 34), "CSI 14 t")));
        assert_eq!(cell_size(&Reply::default(), Some((120, 30)), Some((15, 33))), Some(((15, 33), "window size")));
        assert_eq!(cell_size(&Reply::default(), Some((120, 30)), None), None);
    }

    #[test]
    fn escapes_for_pasting() {
        assert_eq!(escaped(b"\x1b_Gi=31;OK\x1b\\\x07"), "\\e_Gi=31;OK\\e\\\\\\x07");
    }
}
