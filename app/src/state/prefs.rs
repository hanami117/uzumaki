//! What the Settings overlay (Ctrl+,) changes: account, streaming quality, start page, tray,
//! caches. Every setter saves `settings.json` and applies the change at once.

use std::path::{Path, PathBuf};

use super::*;
use crate::backdrop::Slot;
use crate::appearance::{BgSource, Effect, NpBg, ShowOn, opacity_range};

/// The caches Settings → General can clear.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheKind {
    /// Cover images, `~/.cache/uzumaki/art`.
    Art,
    /// The offline library and album/playlist pages, `~/.cache/uzumaki/library`.
    Library,
}

/// Bytes on disk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheSizes {
    pub art: u64,
    pub library: u64,
}

impl CacheSizes {
    pub fn of(&self, kind: CacheKind) -> u64 {
        match kind {
            CacheKind::Art => self.art,
            CacheKind::Library => self.library,
        }
    }
}

impl State {
    // ---------------------------------------------------------------- account

    /// Signs out of Apple Music: the bridge signs MusicKit out and clears the profile's apple.com
    /// cookies; then the library, queue and their disk caches go. Ask before calling (the UI
    /// confirms): signing in again needs the Apple ID password.
    pub fn sign_out(&mut self) {
        if self.session.signing_out {
            return;
        }
        match &self.bridge {
            Some(b) => {
                self.session.signing_out = true;
                b.sign_out(|r| Some(Msg::SignedOut(r.map(|_| ()))));
            }
            None => self.signed_out(), // headless: state only
        }
    }

    /// The bridge confirmed the sign-out.
    pub(super) fn signed_out(&mut self) {
        self.queue = PlayQueue::default();
        self.playback = Playback {
            volume: self.playback.volume,
            shuffle: self.playback.shuffle,
            repeat: self.playback.repeat,
            ..Playback::default()
        };
        self.starting = None;
        self.lyrics.clear();
        self.ui.now_playing = false;
        if let Some(t) = &mut self.tray {
            t.set_song(None);
            t.set_playing(false);
        }
        // The auth event may have done this already; do it regardless.
        self.session.authorized = false;
        self.save(cache::SESSION, false);
        self.resume = None;
        self.drop_account();
        self.toast = Some("Signed out of Apple Music.".into());
    }

    // ---------------------------------------------------------------- playback

    /// Data saver streams at 64 kbps; otherwise 256 kbps AAC (the best Apple serves the web).
    pub fn set_data_saver(&mut self, on: bool) {
        if self.settings.data_saver == on {
            return;
        }
        self.settings.data_saver = on;
        self.save_settings();
        if let Some(b) = &self.bridge {
            b.set_bitrate(if on { 64 } else { 256 });
        }
    }

    /// Lyrics highlight: word by word (when Apple has word timing) or whole lines.
    pub fn set_lyrics_words(&mut self, on: bool) {
        if self.settings.lyrics_words != on {
            self.toggle_lyrics_words();
        }
    }

    // ---------------------------------------------------------------- appearance

    pub fn set_bg_source(&mut self, source: BgSource) {
        if self.settings.appearance.source != source {
            self.settings.appearance.source = source;
            self.save_settings();
        }
    }

    /// Chooses the background picture and shows it.
    pub fn set_bg_picture(&mut self, path: PathBuf) {
        let a = &mut self.settings.appearance;
        a.picture = Some(path);
        a.source = BgSource::Picture;
        self.save_settings();
    }

    /// Clears the chosen picture (the source falls back to Off if it was the picture) and frees
    /// the rendered images.
    pub fn remove_picture(&mut self) {
        let a = &mut self.settings.appearance;
        a.picture = None;
        if a.source == BgSource::Picture {
            a.source = BgSource::Off;
        }
        self.backdrops.hide(Slot::App);
        self.backdrops.hide(Slot::Preview);
        self.save_settings();
    }

    pub fn set_show_on(&mut self, show_on: ShowOn) {
        if self.settings.appearance.show_on != show_on {
            self.settings.appearance.show_on = show_on;
            self.save_settings();
        }
    }

    /// Song-list strength for a theme (`light`) while the slider moves (save with
    /// `save_appearance` on release).
    pub fn set_bg_list_opacity(&mut self, light: bool, opacity: f32) {
        let (min, max) = opacity_range(light);
        let a = &mut self.settings.appearance;
        *if light { &mut a.light_list_opacity } else { &mut a.list_opacity } = opacity.clamp(min, max);
    }

    pub fn set_bg_effect(&mut self, effect: Effect) {
        if self.settings.appearance.effect != effect {
            self.settings.appearance.effect = effect;
            self.save_settings();
        }
    }

    /// Card-page opacity for a theme (`light`) while the slider moves: applies at once, doesn't
    /// save (see `save_appearance`).
    pub fn set_bg_opacity(&mut self, light: bool, opacity: f32) {
        let (min, max) = opacity_range(light);
        let a = &mut self.settings.appearance;
        *if light { &mut a.light_opacity } else { &mut a.opacity } = opacity.clamp(min, max);
    }

    /// Saves after a slider gesture ends (every move would spawn a save thread).
    pub fn save_appearance(&self) {
        self.save_settings();
    }

    pub fn set_bg_fade(&mut self, on: bool) {
        if self.settings.appearance.fade != on {
            self.settings.appearance.fade = on;
            self.save_settings();
        }
    }

    pub fn set_np_bg(&mut self, np: NpBg) {
        if self.settings.appearance.np != np {
            self.settings.appearance.np = np;
            self.save_settings();
        }
    }

    pub fn set_np_effect(&mut self, effect: Effect) {
        if self.settings.appearance.np_effect != effect {
            self.settings.appearance.np_effect = effect;
            self.save_settings();
        }
    }

    /// After every page (re)load MusicKit is back at 256 kbps and the bridge at repeat off.
    pub(super) fn reapply_playback_prefs(&self) {
        let Some(b) = &self.bridge else { return };
        if self.settings.data_saver {
            b.set_bitrate(64);
        }
        if self.playback.repeat != Repeat::Off {
            b.set_repeat(self.playback.repeat.bridge_name());
        }
    }

    // ---------------------------------------------------------------- general

    pub fn set_start_page(&mut self, page: StartPage) {
        if self.settings.start_page == page {
            return;
        }
        self.settings.start_page = page;
        self.settings.last_route = (page == StartPage::Last).then(|| self.route.clone());
        self.save_settings();
    }

    /// Keeps `last_route` current while the start page is "last page".
    pub(super) fn remember_route(&mut self) {
        if self.settings.start_page == StartPage::Last && self.settings.last_route.as_ref() != Some(&self.route) {
            self.settings.last_route = Some(self.route.clone());
            self.save_settings();
        }
    }

    /// Shows or removes the tray icon. Close to tray needs it.
    pub fn set_tray_icon(&mut self, on: bool) {
        if self.settings.tray_icon == on {
            return;
        }
        self.settings.tray_icon = on;
        self.save_settings();
        if !on {
            self.tray = None; // dropping it removes the icon
        } else if self.tray.is_none() {
            self.start_tray();
        }
    }

    /// Starts the tray icon with the current song and the resolved app icon.
    fn start_tray(&mut self) {
        let icon = self.shown_app_icon();
        if let Some(h) = &self.handle {
            self.tray = TrayCtl::start(h.clone(), icon);
        }
        if let Some(t) = &mut self.tray {
            t.set_song(self.playback.song.as_ref());
            t.set_playing(self.playback.state.is_active());
        }
    }

    /// Notify on song change (Settings → General). Switching it on asks the system for
    /// permission where that is needed (macOS).
    pub fn set_notify_song_change(&mut self, on: bool) {
        if self.settings.notify_song_change != on {
            self.settings.notify_song_change = on;
            self.notifier.set_enabled(on);
            self.save_settings();
        }
    }

    /// `App::on_focus`: the window gained or lost focus (a hidden or minimised one lost it).
    pub fn set_window_focused(&mut self, focused: bool) {
        self.notifier.set_focused(focused);
    }

    /// Closing the window hides it to the tray (music keeps playing) instead of quitting.
    pub fn set_close_to_tray(&mut self, on: bool) {
        if self.settings.close_to_tray != on {
            self.settings.close_to_tray = on;
            self.save_settings();
        }
    }

    /// `App::on_close`: true when the window should just hide (close to tray, or started in
    /// the background).
    pub fn hide_on_close(&mut self) -> bool {
        let hide = self.background || (self.settings.closes_to_tray() && self.tray.is_some());
        if hide {
            // Synced lyrics keep `poll` running every frame; nobody can see them now.
            crate::ui::close_now_playing(self);
            // The animated covers too (Now Playing's parked one included): no frame may come
            // to stop them.
            self.motion.stop();
            self.motion.stop_page();
            // macOS: no window and a menu bar icon to get back in, so the Dock icon goes too.
            // `tray::show_window` brings it back. On a Quit/Cmd-Q the app is already ending
            // (Rune runs this hook then too), so the request is never applied.
            #[cfg(target_os = "macos")]
            if let (Some(h), true) = (&self.handle, self.tray.is_some()) {
                h.set_dock_icon(false);
            }
        }
        hide
    }

    /// `App::on_reopen` (macOS: Dock icon clicked, or the app opened again from Finder): with no
    /// window showing, bring the window and the Dock icon back. Rune's own default does nothing
    /// while the Dock icon is hidden, which is the state close to tray leaves.
    #[cfg(target_os = "macos")]
    pub fn reopen(&mut self, has_visible_windows: bool) -> bool {
        match (&self.handle, has_visible_windows) {
            (Some(h), false) => {
                crate::tray::show_window(h);
                true
            }
            _ => false,
        }
    }

    /// Measures the caches on a background thread; `cache_sizes` updates when done.
    pub fn refresh_cache_sizes(&self) {
        let Some(h) = self.handle.clone() else { return };
        std::thread::spawn(move || {
            // Artwork counts the animated artwork's clips too (they sit beside the covers).
            let art = dir_size(&cache_dir(CacheKind::Art)) + dir_size(&crate::motion::clips::dir());
            let sizes = CacheSizes { art, library: dir_size(&cache_dir(CacheKind::Library)) };
            h.update(move |s: &mut State, tx| {
                tx.instant();
                s.cache_sizes = Some(sizes);
            });
        });
    }

    /// Deletes one cache's files on a background thread, then measures again. What's on screen
    /// stays (it's in memory); the files come back as things are fetched again.
    pub fn clear_cache(&mut self, kind: CacheKind) {
        if let Some(sizes) = &mut self.cache_sizes {
            match kind {
                CacheKind::Art => sizes.art = 0,
                CacheKind::Library => sizes.library = 0,
            }
        }
        if kind == CacheKind::Library {
            self.coll_cache.clear();
        }
        let Some(h) = self.handle.clone() else { return };
        std::thread::spawn(move || {
            remove_contents(&cache_dir(kind));
            if kind == CacheKind::Art {
                remove_contents(&crate::motion::clips::dir());
            }
            h.update(|s: &mut State, _| s.refresh_cache_sizes());
        });
    }
}

fn cache_dir(kind: CacheKind) -> PathBuf {
    match kind {
        CacheKind::Art => crate::art::cache_dir(),
        CacheKind::Library => Cache::default_location().dir().to_path_buf(),
    }
}

/// Total size of the files under `dir` (recursive; symlinks not followed).
fn dir_size(dir: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map_or(0, |m| m.len()),
            _ => 0,
        })
        .sum()
}

/// Deletes everything inside `dir`, keeping `dir` itself.
fn remove_contents(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let _ = match e.file_type() {
            Ok(t) if t.is_dir() => std::fs::remove_dir_all(e.path()),
            _ => std::fs::remove_file(e.path()),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dir_size_and_remove_contents() {
        let dir = std::env::temp_dir().join(format!("uzumaki-prefs-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("collections")).unwrap();
        std::fs::write(dir.join("a.json"), [0u8; 100]).unwrap();
        std::fs::write(dir.join("collections/b.json"), [0u8; 50]).unwrap();
        assert_eq!(dir_size(&dir), 150);
        remove_contents(&dir);
        assert!(dir.exists());
        assert_eq!(dir_size(&dir), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn appearance_setters() {
        let mut s = State::offline();
        s.set_bg_source(BgSource::Builtin);
        s.set_bg_effect(Effect::Ascii);
        s.set_bg_fade(false);
        s.set_np_bg(NpBg::App);
        s.set_np_effect(Effect::Dither);
        let a = &s.settings.appearance;
        assert_eq!((a.source, a.effect, a.fade, a.np, a.np_effect), (BgSource::Builtin, Effect::Ascii, false, NpBg::App, Effect::Dither));
        // Choosing a picture also switches the source to it.
        s.set_bg_picture(PathBuf::from("/tmp/x.png"));
        assert_eq!(s.settings.appearance.source, BgSource::Picture);
        assert_eq!(s.settings.appearance.picture.as_deref(), Some(Path::new("/tmp/x.png")));
        // Opacity is clamped to the slider range.
        use crate::appearance::{LIGHT_OPACITY_DEFAULT, LIGHT_OPACITY_MAX, OPACITY_MAX, OPACITY_MIN};
        s.set_bg_opacity(false, 5.0);
        assert_eq!(s.settings.appearance.opacity, OPACITY_MAX);
        s.set_bg_opacity(false, -1.0);
        assert_eq!(s.settings.appearance.opacity, OPACITY_MIN);
        assert_eq!(s.settings.appearance.light_opacity, LIGHT_OPACITY_DEFAULT, "the light strength is its own");
        s.set_bg_opacity(true, 5.0);
        assert_eq!(s.settings.appearance.light_opacity, LIGHT_OPACITY_MAX);
        assert_eq!(s.settings.appearance.opacity, OPACITY_MIN);
        s.set_show_on(ShowOn::Cards);
        assert_eq!(s.settings.appearance.show_on, ShowOn::Cards);
        s.set_bg_list_opacity(false, 9.0);
        assert_eq!(s.settings.appearance.list_opacity, OPACITY_MAX);
        s.set_bg_list_opacity(false, 0.0);
        assert_eq!(s.settings.appearance.list_opacity, OPACITY_MIN);
        s.set_bg_list_opacity(true, 0.7);
        assert_eq!((s.settings.appearance.light_list_opacity, s.settings.appearance.list_opacity), (0.7, OPACITY_MIN));
    }

    #[test]
    fn remove_picture_resets_source() {
        let mut s = State::offline();
        s.set_bg_picture(PathBuf::from("/tmp/x.png"));
        s.remove_picture();
        assert_eq!((s.settings.appearance.picture.clone(), s.settings.appearance.source), (None, BgSource::Off));
        // Another source stays.
        s.set_bg_picture(PathBuf::from("/tmp/x.png"));
        s.set_bg_source(BgSource::Builtin);
        s.remove_picture();
        assert_eq!((s.settings.appearance.picture.is_none(), s.settings.appearance.source), (true, BgSource::Builtin));
    }
}

// ---------------------------------------------------------------- shortcuts and table views
impl State {
    /// Sets the shortcut of action `id` ("" removes it). Setting the default chord forgets the
    /// change. `ui::keys::record` resolves clashes before calling this.
    pub fn set_keybind(&mut self, id: &str, chord: &str, default: &str) {
        if chord == default {
            self.settings.keybinds.remove(id);
        } else {
            self.settings.keybinds.insert(id.to_string(), chord.to_string());
        }
        self.save_settings();
    }

    pub fn reset_keybind(&mut self, id: &str) {
        if self.settings.keybinds.remove(id).is_some() {
            self.save_settings();
        }
    }

    pub fn reset_keybinds(&mut self) {
        self.settings.keybinds.clear();
        self.save_settings();
    }

    pub fn set_number_shortcuts(&mut self, m: crate::settings::NumberMod, k: crate::settings::NumberKeys) {
        self.settings.number_mod = m;
        self.settings.number_keys = k;
        self.save_settings();
    }

    /// Saves the songs-table view and sorts after the user changed them.
    pub fn remember_table(&mut self) {
        let v = self.ui.table.view();
        if self.settings.table != v {
            self.settings.table = v;
            self.save_settings();
        }
    }
}

// ---------------------------------------------------------------- appearance
impl State {
    pub fn set_startup_animation(&mut self, on: bool) {
        self.settings.startup_animation = on;
        self.save_settings();
    }

    pub fn set_player_style(&mut self, style: crate::settings::PlayerStyle) {
        self.settings.player_style = style;
        self.save_settings();
    }

    pub fn set_theme(&mut self, theme: crate::settings::Theme) {
        if self.settings.theme != theme {
            self.settings.theme = theme;
            self.save_settings();
            self.sync_app_icon(); // App icon → Follow theme
        }
    }

    /// Picks the desktop's app icon: saved now, then applied as `sync_app_icon` says.
    pub fn set_app_icon(&mut self, icon: crate::settings::AppIcon) {
        if self.settings.app_icon != icon {
            self.settings.app_icon = icon;
            self.save_settings();
            self.sync_app_icon();
        }
    }

    /// The view saw the OS appearance (`Cx::system_dark`); called through the handle only when it
    /// differs from the last one (`report_system_dark`).
    pub fn set_system_dark(&mut self, dark: bool) {
        if self.system_dark != Some(dark) {
            self.system_dark = Some(dark);
            self.sync_app_icon();
        }
    }

    /// From `ui::view`, which alone knows the OS appearance: hands a change to `set_system_dark`
    /// on the next turn (the view can't change state). Nothing in headless runs and tests.
    pub fn report_system_dark(&self, dark: bool) {
        if self.system_dark != Some(dark) {
            if let Some(h) = &self.handle {
                h.update_if(move |s: &mut State, _tx| {
                    s.set_system_dark(dark);
                    false // nothing on screen depends on it
                });
            }
        }
    }

    /// The icon the desktop should show now: the App icon setting resolved against the theme and
    /// the OS appearance (Dark while Follow theme waits for the first frame).
    pub fn shown_app_icon(&self) -> crate::settings::IconVariant {
        self.settings.app_icon.resolve(self.settings.theme, self.system_dark).unwrap_or(crate::settings::IconVariant::Dark)
    }

    /// Applies the resolved app icon, only when it differs from the one last applied (so a theme
    /// change that keeps the icon does nothing). Linux: the installed icon files on a background
    /// thread (`app_icon::apply_async`); the tray at the next start, except a tray started on a
    /// guess before Follow theme was known, which is started again once. macOS: the menu bar icon
    /// now, the .app bundle's icon on a background thread (`bundle_icon::apply_async`); the Dock
    /// follows from the view (`dock_icon::sync`). Offline states (tests, headless) touch no files.
    pub(super) fn sync_app_icon(&mut self) {
        let Some(icon) = self.settings.app_icon.resolve(self.settings.theme, self.system_dark) else {
            return; // Follow theme, appearance not reported yet
        };
        if self.applied_icon == Some(icon) {
            return;
        }
        self.applied_icon = Some(icon);
        #[cfg(test)]
        {
            self.icon_applies += 1;
        }
        let guessed = std::mem::take(&mut self.tray_icon_guessed);
        if cfg!(target_os = "linux") && guessed && icon != crate::settings::IconVariant::Dark && self.tray.is_some() {
            self.tray = None; // Rune's tray can't change its icon: start it again with the right one
            self.start_tray();
        }
        if let Some(t) = &mut self.tray {
            t.set_app_icon(icon);
        }
        #[cfg(target_os = "linux")]
        if self.persist_settings {
            crate::app_icon::apply_async(icon);
        }
        #[cfg(target_os = "macos")]
        if self.persist_settings {
            crate::bundle_icon::apply_async(icon);
        }
    }

    /// Shows or hides sidebar entry `id` (`ui::sidebar::NAV_IDS`).
    pub fn set_nav_hidden(&mut self, id: &str, hidden: bool) {
        self.settings.hidden_nav.retain(|x| x != id);
        if hidden {
            self.settings.hidden_nav.push(id.to_string());
        }
        self.save_settings();
    }

    pub fn nav_hidden(&self, id: &str) -> bool {
        self.settings.hidden_nav.iter().any(|x| x == id)
    }
}
