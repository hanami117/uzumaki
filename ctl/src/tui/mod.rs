//! The interactive screens (`uzu` alone in a terminal): the loop and the terminal. State and
//! keys are in `app` (queue), `browse` (playlists, albums) and `search`; drawing in `view` and
//! `table`; slow requests in `jobs`; the song's cover in `cover`.

mod app;
mod browse;
mod cover;
mod jobs;
mod lyrics;
mod probe;
mod search;
mod showcase;
pub(crate) mod sort;
mod table;
mod view;
#[cfg(test)]
mod tests;

use std::io::{self, Write, stdout};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind};
use ratatui::crossterm::execute;
use uzumaki_ctl::{Client, Reply, Request};

use crate::run::{Backend, Env, Fail};
use app::App;
use jobs::Threaded;

/// How long to wait for input before looping (keeps the CPU idle).
pub(super) const WAIT: Duration = Duration::from_millis(100);
/// Redraw at least this often, for the clock and the bar.
const REDRAW: Duration = Duration::from_millis(250);

/// The app's socket, reconnecting after it was lost (Uzumaki restarted).
pub(super) struct Reconnecting {
    pub(super) client: Option<Client>,
}

impl Backend for Reconnecting {
    fn request(&mut self, req: &Request) -> io::Result<Reply> {
        if self.client.is_none() {
            self.client = Some(Client::connect()?);
        }
        let reply = self.client.as_mut().map(|c| c.request(req)).unwrap_or_else(|| Err(io::ErrorKind::NotConnected.into()));
        if reply.is_err() {
            self.client = None;
        }
        reply
    }
}

pub fn run(client: Client, env: &Env) -> Result<(), Fail> {
    let mut terminal = ratatui::try_init().map_err(|e| Fail::err(format!("can't use the terminal: {e}")))?;
    let _ = execute!(stdout(), EnableMouseCapture);
    // ratatui's panic hook restores the screen; also give the mouse back.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(stdout(), DisableMouseCapture);
        previous(info);
    }));
    let result = event_loop(&mut terminal, &mut Reconnecting { client: Some(client) }, env);
    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
    result.map_err(|e| Fail::err(format!("terminal error: {e}")))
}

fn event_loop(terminal: &mut DefaultTerminal, be: &mut dyn Backend, env: &Env) -> io::Result<()> {
    let mut app = App::new(Instant::now(), Box::new(Threaded::new()));
    app.last = env.last.clone();
    app.set_config(crate::config::Config::load(), Instant::now());
    app.cover = cover::Cover::new(cover::Mode::from_env());
    app.layout = showcase::Layout::from_env();
    if let Some(path) = sort::state_path() {
        app.use_sort_file(path);
    }
    app.start(be, std::env::var_os("UZUMAKI_VIEW").is_some(), Instant::now());
    let result = screens(terminal, be, env, &mut app);
    // Take our kitty image out of the terminal before handing it back.
    app.cover.clear();
    write_escapes(&app.cover.take_deletes());
    result
}

/// `uzu cover-info`: the cover's detection exactly as the screens run it (raw mode, the same
/// query), as text to paste into a bug report. Needs a terminal; the app isn't involved.
pub fn cover_info() -> Result<String, Fail> {
    use std::io::IsTerminal;
    use ratatui::crossterm::terminal::{disable_raw_mode, enable_raw_mode};
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(Fail::err("cover-info asks the terminal, so run it in one (not piped)"));
    }
    let mode = cover::Mode::from_env();
    let mode = if mode == cover::Mode::Off { cover::Mode::Auto } else { mode };
    enable_raw_mode().map_err(|e| Fail::err(format!("can't use the terminal: {e}")))?;
    let found = cover::detect(mode);
    let _ = disable_raw_mode();
    Ok(cover::report(&found, std::env::var("UZUMAKI_COVER").ok().as_deref()))
}

/// Writes escapes ratatui doesn't know about (deleting kitty images) straight to the terminal.
fn write_escapes(text: &str) {
    if !text.is_empty() {
        let mut out = stdout();
        let _ = out.write_all(text.as_bytes()).and_then(|()| out.flush());
    }
}

fn screens(terminal: &mut DefaultTerminal, be: &mut dyn Backend, env: &Env, app: &mut App) -> io::Result<()> {
    let mut last_draw: Option<Instant> = None;
    while !app.quit && !env.stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        let size = terminal.size()?;
        app.resize_to(size.width, size.height);
        let mut dirty = app.tick(be, now);
        // Normally WAIT; on the lyrics screen, until the next line or word starts.
        if event::poll(app.wait(now))? {
            loop {
                match event::read()? {
                    Event::Key(k) if k.kind != KeyEventKind::Release => app.handle_key(k, be, Instant::now()),
                    Event::Mouse(m) => app.handle_mouse(m, be, Instant::now()),
                    Event::Resize(w, h) => {
                        app.resize_to(w, h);
                        app.cover.window_changed(cover::font_from_window());
                        app.sync_cover();
                    }
                    _ => {}
                }
                dirty = true;
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
        // Ask the terminal what it can draw the first time the cover shows (at the start, or at
        // the first `i`): the query reads its answer from stdin, so not while reading keys.
        if app.cover.needs_detect() {
            app.cover.set_caps(cover::detect(app.cover.detect_mode()).caps);
            app.sync_cover();
            dirty = true;
        }
        let now = Instant::now();
        if dirty || last_draw.is_none_or(|t| now.saturating_duration_since(t) >= REDRAW) {
            write_escapes(&app.cover.take_deletes());
            // A new view: start from a blank screen, so no sixel or iTerm2 picture stays behind.
            if std::mem::take(&mut app.redraw) {
                terminal.clear()?;
            }
            terminal.draw(|f| view::draw(f, app, now))?;
            last_draw = Some(now);
        }
    }
    Ok(())
}
