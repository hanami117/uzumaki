//! The update notice's state side (`crate::update` has the check itself).

use super::State;
use crate::update::{self, Install, Record};

/// What the automatic check remembers while the app runs.
#[derive(Debug, Default)]
pub struct AutoCheck {
    /// The hourly timer thread was started (once).
    timer: bool,
    /// An automatic check's worker is out and hasn't answered.
    running: bool,
    /// The version whose notice was closed with ✕: "until the next start" holds for the timer too.
    dismissed: Option<String>,
}

impl State {
    /// Starts the day's update check on a worker thread, when it is on. Only the real app has
    /// a handle, so headless runs and tests never fetch. A test instance doesn't either, unless
    /// `UZUMAKI_UPDATE_URL` points it somewhere.
    pub fn start_update_check(&mut self) {
        let Some(handle) = self.handle.clone() else { return };
        if !may_fetch() || self.update_auto.running {
            return;
        }
        self.update_auto.running = true;
        // Switched off, the worker only reads when the last check ran (Settings shows it).
        update::spawn(self.settings.check_updates, move |rec| {
            let install = update::this_install();
            handle.update(move |s: &mut State, _tx| s.update_known(&rec, install));
        });
    }

    /// While the app stays open (the tray): wakes once an hour on its own thread, and the start-up
    /// rules decide each time (`tick_update_check`). Nothing runs in headless runs and tests (no
    /// handle) or in a test instance; started once.
    pub fn start_update_timer(&mut self) {
        let Some(handle) = self.handle.clone() else { return };
        if self.update_auto.timer || !may_fetch() {
            return;
        }
        self.update_auto.timer = true;
        std::thread::Builder::new()
            .name("update-timer".into())
            .spawn(move || loop {
                std::thread::sleep(update::RECHECK);
                // False: a tick that finds nothing due changes nothing the view reads, so it costs
                // no frame (a check that finds something reports through `update_known`).
                handle.update_if(|s: &mut State, _tx| {
                    s.tick_update_check(update::now());
                    false
                });
            })
            .ok();
    }

    /// One tick of the timer at `now`: the day's check when it is due.
    pub fn tick_update_check(&mut self, now: u64) {
        if self.auto_check_wanted(now) {
            self.start_update_check();
        }
    }

    /// Whether the timer should check at `now` (`update::auto_wanted` with this state).
    pub fn auto_check_wanted(&self, now: u64) -> bool {
        update::auto_wanted(
            self.settings.check_updates,
            self.update.is_some(),
            self.update_auto.running || self.update_check.running,
            self.update_check.last_check,
            now,
        )
    }

    /// What the check knows: show the notice if a newer, unskipped version is out.
    pub fn update_known(&mut self, rec: &Record, install: Install) {
        self.update_auto.running = false;
        self.update_check.last_check = rec.last_check;
        if !self.settings.check_updates {
            return;
        }
        let dismissed = self.update_auto.dismissed.as_deref();
        self.update = rec
            .latest
            .as_ref()
            .and_then(|l| update::notice(l, crate::VERSION, rec.skipped.as_deref(), install, cfg!(target_os = "macos")))
            .filter(|n| dismissed != Some(n.version.as_str()));
    }

    /// Settings → General → Check for updates. On: check now (still at most once a day). Off:
    /// the notice goes.
    pub fn set_check_updates(&mut self, on: bool) {
        if self.settings.check_updates == on {
            return;
        }
        self.settings.check_updates = on;
        self.save_settings();
        if on {
            self.start_update_check();
        } else {
            self.update = None;
        }
    }

    /// "Check for Updates…" (the macOS app menu, the tray / menu bar icon) and Settings →
    /// General → Check now: asks now, whatever the day's limit, the switch or a skipped
    /// version, and always answers (`manual_answer`).
    pub fn check_updates_now(&mut self) {
        if self.update_check.running {
            return;
        }
        let Some(handle) = self.handle.clone() else { return };
        if !may_fetch() {
            self.manual_answer(Err("off in test instances".into()), self.update_check.last_check, Install::Direct);
            return;
        }
        self.update_check.running = true;
        self.update_check.result = None;
        update::spawn_manual(move |answer, last_check| {
            let install = update::this_install();
            handle.update(move |s: &mut State, _tx| s.manual_answer(answer, last_check, install));
        });
    }

    /// A manual check's answer: a newer version shows the notice (even a skipped one); up to
    /// date and failures say so (a toast, unless Settings is open, which shows it in place).
    pub fn manual_answer(&mut self, answer: Result<update::Latest, String>, last_check: Option<u64>, install: Install) {
        self.update_check.running = false;
        self.update_check.last_check = last_check.or(self.update_check.last_check);
        let notice = answer.as_ref().ok().and_then(|l| update::notice(l, crate::VERSION, None, install, cfg!(target_os = "macos")));
        let line = match (&answer, &notice) {
            (_, Some(n)) => n.message(),
            (Ok(_), None) => update::UP_TO_DATE.to_string(),
            (Err(_), None) => update::CHECK_FAILED.to_string(),
        };
        if notice.is_some() {
            self.update = notice;
            self.update_auto.dismissed = None; // asked for by hand: it shows
        } else if self.ui.settings.open.is_none() {
            self.toast = Some(line.clone());
        }
        self.update_check.result = Some(line);
    }

    /// ✕ on the notice: gone until the next start.
    pub fn dismiss_update(&mut self) {
        self.update_auto.dismissed = self.update.take().map(|n| n.version);
    }

    /// "Skip this version": gone, and this version never shows again (a newer one will).
    pub fn skip_update(&mut self) {
        if let Some(n) = self.update.take() {
            if self.handle.is_some() {
                update::skip(n.version);
            }
        }
    }
}

/// A test instance never fetches unless `UZUMAKI_UPDATE_URL` points it somewhere.
fn may_fetch() -> bool {
    let var = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
    !(var("UZUMAKI_TEST_INSTANCE") && !var("UZUMAKI_UPDATE_URL"))
}

#[cfg(test)]
mod tests {
    use rune::Runtime;

    use crate::state::State;
    use crate::update::Install;

    fn window(scene: &str) -> Runtime<State> {
        let mut s = State::offline();
        (crate::scenes::SCENES.iter().find(|(n, _)| *n == scene).unwrap().1)(&mut s);
        let mut rt = Runtime::new(s, crate::ui::view, (1360.0, 860.0), rune::Env { system_dark: true, scale: 1.0 });
        settle(&mut rt);
        rt
    }

    fn settle(rt: &mut Runtime<State>) {
        rt.frame(0.0);
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
    }

    fn click(rt: &mut Runtime<State>, key: &str) {
        let r = rt.presented(key).unwrap_or_else(|| panic!("{key} on screen")).0;
        let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        settle(rt);
    }

    #[test]
    fn a_hand_install_gets_whats_new_and_download() {
        let rt = window("update-notice");
        for key in ["update-notice", "update-notes", "update-download", "update-skip", "update-dismiss"] {
            assert!(rt.presented(key).is_some(), "{key}");
        }
        assert_eq!(rt.state.update.as_ref().unwrap().install, Install::Direct);
    }

    #[test]
    fn package_installs_have_no_download_button() {
        for scene in ["update-notice-package", "update-notice-homebrew"] {
            let rt = window(scene);
            assert!(rt.presented("update-notice").is_some(), "{scene}");
            assert!(rt.presented("update-notes").is_some(), "{scene}: What's new stays");
            assert!(rt.presented("update-download").is_none(), "{scene}: no Download");
            assert!(rt.state.update.as_ref().unwrap().message().contains("Update with your package manager"));
        }
    }

    #[test]
    fn skip_and_dismiss_hide_the_notice() {
        for button in ["update-skip", "update-dismiss"] {
            let mut rt = window("update-notice");
            click(&mut rt, button);
            assert!(rt.state.update.is_none(), "{button}");
            assert!(rt.presented("update-notice").is_none(), "{button}");
        }
    }

    #[test]
    fn the_switch_off_hides_it_and_keeps_it_hidden() {
        let mut rt = window("update-notice");
        let rec = crate::update::Record {
            latest: Some(crate::update::Latest { version: "9.0.0".into(), ..Default::default() }),
            ..Default::default()
        };
        rt.update(|s, _| s.set_check_updates(false));
        settle(&mut rt);
        assert!(!rt.state.settings.check_updates);
        assert!(rt.presented("update-notice").is_none());
        // A check that was already running when it was switched off shows nothing.
        rt.update(move |s, _| s.update_known(&rec, Install::Direct));
        assert!(rt.state.update.is_none());
    }

    fn latest(v: &str) -> crate::update::Latest {
        crate::update::Latest { version: v.into(), notes_url: "https://getuzumaki.app/notes".into(), ..Default::default() }
    }

    #[test]
    fn a_manual_check_always_answers() {
        let mut s = State::offline();
        s.manual_answer(Ok(latest(crate::VERSION)), Some(1_800_000_000), Install::Direct);
        assert_eq!(s.toast.as_deref(), Some("Uzumaki is up to date."));
        assert!(s.update.is_none());
        assert_eq!(s.update_check.last_check, Some(1_800_000_000));
        let mut s = State::offline();
        s.manual_answer(Err("offline".into()), None, Install::Direct);
        assert_eq!(s.toast.as_deref(), Some("Couldn't check for updates."));
        assert!(!s.update_check.running);
    }

    #[test]
    fn a_manual_check_shows_a_newer_version_even_skipped_or_switched_off() {
        let mut s = State::offline();
        s.settings.check_updates = false;
        s.manual_answer(Ok(latest("99.0.0")), None, Install::Direct);
        let n = s.update.as_ref().expect("the notice");
        assert_eq!(n.message(), "Uzumaki 99.0.0 is available.");
        assert!(s.toast.is_none(), "the notice is the answer");
        assert_eq!(s.update_check.result.as_deref(), Some("Uzumaki 99.0.0 is available."));
        for install in [Install::System, Install::Homebrew] {
            let mut s = State::offline();
            s.manual_answer(Ok(latest("99.0.0")), None, install);
            assert!(s.update.as_ref().unwrap().download.is_none(), "{install:?}: no Download");
            assert!(s.update_check.result.as_ref().unwrap().contains("Update with your package manager"));
        }
    }

    #[test]
    fn settings_shows_check_now_with_the_answer_in_place() {
        let mut rt = window("home");
        rt.update(|s, _| crate::ui::settings::open_tab(s, crate::ui::settings::Tab::General));
        settle(&mut rt);
        rt.update(|_, tx| tx.scroll_center(crate::ui::settings_search::Row::CheckNow.key()));
        settle(&mut rt);
        assert!(rt.presented("settings/check-now").is_some());
        rt.update(|s, _| s.manual_answer(Ok(latest(crate::VERSION)), None, Install::Direct));
        settle(&mut rt);
        assert!(rt.state.toast.is_none(), "Settings is open: the answer shows in the row");
        let texts: Vec<String> = rt.elements().into_iter().filter_map(|e| e.text).collect();
        assert!(texts.iter().any(|t| t.starts_with("Uzumaki is up to date.")), "{texts:?}");
    }

    #[test]
    fn check_for_updates_follows_about_in_the_app_menu() {
        let s = State::offline();
        let m = crate::ui::menu_bar::model(&s);
        let ids: Vec<&str> = m.iter().map(|e| e.id).collect();
        let about = ids.iter().position(|i| *i == "app.about").unwrap();
        assert_eq!(ids[about + 1], "app.check_updates");
        assert!(m[about + 1].enabled);
    }

    fn record(v: &str, last: u64) -> crate::update::Record {
        crate::update::Record { last_check: Some(last), latest: Some(latest(v)), ..Default::default() }
    }

    #[test]
    fn the_timer_asks_again_a_day_after_the_last_check() {
        use crate::update::{DAY, RECHECK};
        let t0 = 1_800_000_000;
        let mut s = State::offline();
        s.update_known(&record(crate::VERSION, t0), Install::Direct); // the start-up check's answer
        let mut checks = 0;
        for hour in 1..=50 {
            let now = t0 + hour * RECHECK.as_secs();
            if s.auto_check_wanted(now) {
                checks += 1;
                s.update_known(&record(crate::VERSION, now), Install::Direct);
            }
        }
        assert_eq!(checks, 2, "hours 24 and 48 of a fake clock");
        assert!(!s.auto_check_wanted(t0 + 50 * RECHECK.as_secs()));
        assert!(s.auto_check_wanted(t0 + 50 * RECHECK.as_secs() + DAY));
    }

    #[test]
    fn the_timer_keeps_the_start_up_rules() {
        let t0 = 1_800_000_000;
        let late = t0 + 3 * crate::update::DAY;
        let mut s = State::offline();
        s.update_known(&record(crate::VERSION, t0), Install::Direct);
        assert!(s.auto_check_wanted(late));
        // The off switch.
        s.settings.check_updates = false;
        assert!(!s.auto_check_wanted(late));
        s.settings.check_updates = true;
        // A notice already showing is not asked for again.
        s.update_known(&record("99.0.0", t0), Install::Direct);
        assert!(s.update.is_some());
        assert!(!s.auto_check_wanted(late));
        // Closed with ✕: the same version stays closed when a later check finds it again, a newer
        // one shows.
        s.dismiss_update();
        assert!(s.auto_check_wanted(late));
        s.update_known(&record("99.0.0", late), Install::Direct);
        assert!(s.update.is_none(), "dismissed until the next start");
        s.update_known(&record("99.1.0", late), Install::Direct);
        assert_eq!(s.update.as_ref().unwrap().version, "99.1.0");
        // A skipped version never shows (the record says so), with the package-manager wording kept.
        let mut s = State::offline();
        let mut rec = record("99.0.0", t0);
        rec.skipped = Some("99.0.0".into());
        s.update_known(&rec, Install::System);
        assert!(s.update.is_none());
        rec.skipped = None;
        s.update_known(&rec, Install::System);
        let n = s.update.as_ref().unwrap();
        assert!(n.download.is_none() && n.message().contains("Update with your package manager"));
        // A manual check running holds the timer back; asked for by hand, a dismissed notice shows.
        let mut s = State::offline();
        s.update_check.running = true;
        assert!(!s.auto_check_wanted(late));
        s.update_auto.dismissed = Some("99.0.0".into());
        s.manual_answer(Ok(latest("99.0.0")), Some(late), Install::Direct);
        assert!(s.update.is_some());
    }

    #[test]
    fn headless_and_tests_never_start_a_timer_or_a_check() {
        let mut s = State::offline(); // no handle, like every test and headless run
        s.start_update_timer();
        s.tick_update_check(u64::MAX / 2);
        s.start_update_check();
        assert!(!s.update_auto.timer && !s.update_auto.running, "nothing was started");
    }

    #[test]
    fn on_by_default_and_old_settings_files_get_it_on() {
        assert!(crate::settings::Settings::default().check_updates);
        assert!(crate::settings::Settings::from_json(br#"{"onboarded": 3}"#).check_updates);
        assert!(!crate::settings::Settings::from_json(br#"{"onboarded": 3, "check_updates": false}"#).check_updates);
    }
}
