//! The update notice.
//!
//! At start, and again while the app stays open whenever a day has passed (a timer wakes once an
//! hour and asks `auto_wanted`), at most once a day, a worker thread reads `latest.json` from getuzumaki.app: a plain
//! GET with the User-Agent `Uzumaki/<version>`, nothing else sent. A newer version (semver) that
//! the user hasn't skipped shows a quiet banner (`ui::banners`): "What's new", "Download" (opens
//! the browser), "Skip this version", and ✕ (until the next start). Nothing is ever downloaded
//! by the app. Installed by a package manager (Linux: the binary under `/usr`; macOS: a Homebrew
//! cask), the banner says to update with it and has no Download button.
//!
//! When the last check ran, its answer and the skipped version live in
//! `$XDG_STATE_HOME/uzumaki/update.json` (else `~/.local/state/…`, on macOS too): state, not
//! settings. The switch is `Settings::check_updates` (Settings → General). Headless runs and
//! tests never fetch: only the real app's `State` (with a handle) starts the worker.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Where the latest release is described. `UZUMAKI_UPDATE_URL` overrides it (testing).
pub const LATEST_URL: &str = "https://getuzumaki.app/latest.json";
/// At most one check per this many seconds.
pub const DAY: u64 = 86_400;
/// While the app runs, the timer looks this often at whether the automatic check is due. A
/// wall-clock comparison against `last_check`, so a computer that slept through the day is
/// caught within the hour after it wakes.
pub const RECHECK: Duration = Duration::from_secs(3600);
/// The whole request (connect, TLS, body); a slow site never holds anything up.
const TIMEOUT: Duration = Duration::from_secs(6);

/// `latest.json`: `{version, build, date, notes_url, downloads: {macos_dmg, linux_tarball, appimage, aur, brew}}`.
/// Every field but `version` may be missing (older files have no `appimage` and no `build`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Latest {
    pub version: String,
    /// The build of that version (see `crate::BUILD`). Missing reads as 0, which counts as 1 (`build_of`).
    #[serde(default)]
    pub build: u32,
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub notes_url: String,
    #[serde(default)]
    pub downloads: Downloads,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Downloads {
    pub macos_dmg: String,
    pub linux_tarball: String,
    /// The Linux x86_64 AppImage.
    pub appimage: String,
    pub aur: String,
    pub brew: String,
}

// ---- versions ----

/// A semantic version: `1.2.3`, `v1.2.3`, `1.2.3-beta.2`, `1.2.3+build` (build data ignored).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Vec<String>,
}

impl Version {
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.trim();
        let s = s.strip_prefix('v').unwrap_or(s);
        let s = s.split('+').next()?;
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (s, None),
        };
        let mut nums = core.split('.').map(|n| {
            (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).then(|| n.parse::<u64>().ok()).flatten()
        });
        let (major, minor, patch) = (nums.next()??, nums.next()??, nums.next()??);
        if nums.next().is_some() {
            return None;
        }
        let pre = match pre {
            Some(p) => {
                let ids: Vec<String> = p.split('.').map(str::to_string).collect();
                if ids.iter().any(|i| i.is_empty() || !i.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')) {
                    return None;
                }
                ids
            }
            None => Vec::new(),
        };
        Some(Version { major, minor, patch, pre })
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch)).then_with(|| {
            // A pre-release comes before its release; identifiers compare numerically when both
            // are numbers, numbers before words, and a longer list wins a tie (semver §11).
            match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => {
                    for (a, b) in self.pre.iter().zip(&other.pre) {
                        let o = match (a.parse::<u64>(), b.parse::<u64>()) {
                            (Ok(x), Ok(y)) => x.cmp(&y),
                            (Ok(_), Err(_)) => Ordering::Less,
                            (Err(_), Ok(_)) => Ordering::Greater,
                            (Err(_), Err(_)) => a.cmp(b),
                        };
                        if o != Ordering::Equal {
                            return o;
                        }
                    }
                    self.pre.len().cmp(&other.pre.len())
                }
            }
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A build number as compared: 0 (a file written before builds existed) is build 1.
pub fn build_of(build: u32) -> u32 {
    build.max(1)
}

/// `candidate` (version, build) is newer than `current`: a newer version, or the same version
/// with a higher build. Anything unreadable is never newer.
pub fn is_newer(candidate: (&str, u32), current: (&str, u32)) -> bool {
    match (Version::parse(candidate.0), Version::parse(current.0)) {
        (Some(a), Some(b)) => a.cmp(&b).then(build_of(candidate.1).cmp(&build_of(current.1))) == Ordering::Greater,
        _ => false,
    }
}

// ---- once a day ----

/// What the update check remembers between runs (`update.json`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Record {
    /// Unix seconds of the last attempt, answered or not.
    pub last_check: Option<u64>,
    /// The last good answer, so a start on the same day still shows the notice.
    pub latest: Option<Latest>,
    /// "Skip this version": never show this one again (a newer one shows).
    pub skipped: Option<String>,
    /// The build of the skipped version; missing (older records) counts as 1. A higher build of
    /// the same version shows again.
    pub skipped_build: Option<u32>,
}

impl Record {
    /// The skipped (version, build), if any.
    pub fn skipped(&self) -> Option<(&str, u32)> {
        self.skipped.as_deref().map(|v| (v, build_of(self.skipped_build.unwrap_or(1))))
    }
}

/// A check is due: never checked, a day has passed, or the clock went back.
pub fn due(last_check: Option<u64>, now: u64) -> bool {
    match last_check {
        None => true,
        Some(t) => now < t || now - t >= DAY,
    }
}

/// Whether the timer should start the automatic check now: the switch is on, no notice is
/// showing (or was just dismissed), no check is running, and `due` says a day has passed.
pub fn auto_wanted(enabled: bool, notice_showing: bool, busy: bool, last_check: Option<u64>, now: u64) -> bool {
    enabled && !notice_showing && !busy && due(last_check, now)
}

/// `$XDG_STATE_HOME/uzumaki`, else `~/.local/state/uzumaki` (on macOS too): what the app
/// remembers by itself, apart from the settings.
pub fn state_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(base.join("uzumaki"))
}

/// `$XDG_STATE_HOME/uzumaki/update.json`, else `~/.local/state/uzumaki/update.json`.
pub fn record_path() -> Option<PathBuf> {
    Some(state_dir()?.join("update.json"))
}

impl Record {
    pub fn load(path: &Path) -> Record {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    /// Atomic (temp file, then rename).
    pub fn save(&self, path: &Path) {
        let Ok(json) = serde_json::to_vec_pretty(self) else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

/// The record after a check at `now`: due → `fetch` runs and its answer (if any) is kept;
/// not due → unchanged. Returns whether `fetch` ran.
pub fn check(rec: &mut Record, now: u64, fetch: impl FnOnce() -> Result<Latest, String>) -> bool {
    if !due(rec.last_check, now) {
        return false;
    }
    rec.last_check = Some(now);
    match fetch() {
        Ok(latest) => rec.latest = Some(latest),
        Err(e) => eprintln!("uzumaki: update check failed: {e}"),
    }
    true
}

/// A manual check ("Check for Updates…", Settings → General → Check now): always asks, whatever
/// the day's limit, and the answer counts as the day's check.
pub fn manual(rec: &mut Record, now: u64, fetch: impl FnOnce() -> Result<Latest, String>) -> Result<Latest, String> {
    rec.last_check = Some(now);
    let answer = fetch();
    match &answer {
        Ok(latest) => rec.latest = Some(latest.clone()),
        Err(e) => eprintln!("uzumaki: update check failed: {e}"),
    }
    answer
}

/// Where a manual check stands, for Settings → General.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CheckStatus {
    /// A manual check is running.
    pub running: bool,
    /// Unix seconds of the last check (daily or manual), as `update.json` has it.
    pub last_check: Option<u64>,
    /// The last manual check's answer ("Uzumaki is up to date.", …).
    pub result: Option<String>,
}

pub const UP_TO_DATE: &str = "Uzumaki is up to date.";
pub const CHECK_FAILED: &str = "Couldn't check for updates.";

// ---- "Last checked" ----

/// A moment in local time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i32,
    /// 1–12.
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    /// Days since 1970-01-01 in local time (to tell today and yesterday).
    pub days: i64,
}

/// `unix` in the computer's time zone.
pub fn local_time(unix: u64) -> Option<LocalTime> {
    let t = libc::time_t::try_from(unix).ok()?;
    // SAFETY: localtime_r fills the zeroed `tm` we own; it reads `t` only.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return None;
    }
    let local = unix as i64 + tm.tm_gmtoff as i64;
    Some(LocalTime {
        year: tm.tm_year + 1900,
        month: (tm.tm_mon + 1) as u32,
        day: tm.tm_mday as u32,
        hour: tm.tm_hour as u32,
        minute: tm.tm_min as u32,
        days: local.div_euclid(86_400),
    })
}

/// "today at 14:05", "yesterday at 09:30", "3 Nov at 18:00", "28 Dec 2025 at 18:00".
pub fn when_label(then: LocalTime, now: LocalTime) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let hm = format!("{:02}:{:02}", then.hour, then.minute);
    let month = MONTHS[(then.month.clamp(1, 12) - 1) as usize];
    match now.days - then.days {
        0 => format!("today at {hm}"),
        1 => format!("yesterday at {hm}"),
        _ if then.year == now.year => format!("{} {month} at {hm}", then.day),
        _ => format!("{} {month} {} at {hm}", then.day, then.year),
    }
}

/// Settings' line: "Last checked: today at 14:05", or "Not checked yet".
pub fn last_checked(last: Option<u64>) -> String {
    match last.and_then(|t| Some((local_time(t)?, local_time(now())?))) {
        Some((then, now)) => format!("Last checked: {}", when_label(then, now)),
        None => "Not checked yet".to_string(),
    }
}

// ---- how the app was installed ----

/// Who updates this copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Install {
    /// Installed by hand (DMG, `install.sh`, a build from source): offer the download.
    Direct,
    /// A distribution's package (pacman/AUR, …): the binary is under `/usr`.
    System,
    /// A Homebrew cask.
    Homebrew,
    /// Running as an AppImage (Linux; the `APPIMAGE` variable is set): offer the AppImage.
    AppImage,
}

/// The Homebrew prefixes (Apple silicon, Intel).
const BREW_PREFIXES: [&str; 2] = ["/opt/homebrew", "/usr/local"];

/// How the copy at `exe` (resolved) was installed. `linux`/`mac` pick the rules;
/// `is_dir` answers for the file system (tests pass their own).
///
/// - Linux: under `/usr` but not `/usr/local` (where hand installs go) is a package.
/// - macOS: Homebrew keeps a receipt for every cask in `<prefix>/Caskroom/<token>/.metadata`
///   (with `INSTALL_RECEIPT.json`) while it is installed and moves the app to /Applications, so
///   a bundled copy (`….app/Contents/MacOS/`) with `Caskroom/uzumaki/.metadata` present is the
///   cask's. A `cargo run` build is never counted as one.
pub fn install_kind(exe: &Path, linux: bool, mac: bool, is_dir: impl Fn(&Path) -> bool) -> Install {
    if linux && exe.starts_with("/usr") && !exe.starts_with("/usr/local") {
        return Install::System;
    }
    if mac {
        let bundled = exe.parent().is_some_and(|p| p.ends_with("Contents/MacOS"))
            && exe.ancestors().nth(3).is_some_and(|a| a.extension().is_some_and(|e| e == "app"));
        let in_caskroom = BREW_PREFIXES.iter().any(|p| exe.starts_with(Path::new(p).join("Caskroom")));
        let receipt = BREW_PREFIXES.iter().any(|p| is_dir(&Path::new(p).join("Caskroom/uzumaki/.metadata")));
        if in_caskroom || (bundled && receipt) {
            return Install::Homebrew;
        }
    }
    Install::Direct
}

/// This copy's install kind. An AppImage's binary runs from a temporary mount, so the
/// runtime's `APPIMAGE` variable (the .AppImage file's path) tells it.
pub fn this_install() -> Install {
    if cfg!(target_os = "linux") && std::env::var_os("APPIMAGE").is_some_and(|v| !v.is_empty()) {
        return Install::AppImage;
    }
    let exe = std::env::current_exe().and_then(|p| p.canonicalize()).unwrap_or_default();
    install_kind(&exe, cfg!(target_os = "linux"), cfg!(target_os = "macos"), |p| p.is_dir())
}

// ---- the notice ----

/// What the banner shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub version: String,
    /// The build of `version` (1 and up).
    pub build: u32,
    pub notes_url: String,
    /// The Download button's page; `None` for package installs (and when the file has none).
    pub download: Option<String>,
    pub install: Install,
}

impl Notice {
    /// The banner's line.
    pub fn message(&self) -> String {
        let head = match self.build {
            0 | 1 => format!("Uzumaki {} is available.", self.version),
            b => format!("Uzumaki {} (build {b}) is available.", self.version),
        };
        match self.install {
            Install::Direct | Install::AppImage => head,
            Install::System => format!("{head} Update with your package manager."),
            Install::Homebrew => format!("{head} Update with your package manager (brew upgrade uzumaki)."),
        }
    }
}

/// The notice for `latest`, if it is newer than `current` (version, build) and not skipped.
/// A skip holds for one (version, build): a higher build of the same version shows again.
pub fn notice(latest: &Latest, current: (&str, u32), skipped: Option<(&str, u32)>, install: Install, mac: bool) -> Option<Notice> {
    let build = build_of(latest.build);
    if !is_newer((&latest.version, build), current) || skipped.is_some_and(|(v, b)| same_version(v, &latest.version) && build_of(b) == build) {
        return None;
    }
    let d = &latest.downloads;
    let file = if install == Install::AppImage {
        &d.appimage
    } else if mac {
        &d.macos_dmg
    } else {
        &d.linux_tarball
    };
    let download = match install {
        Install::Direct | Install::AppImage => Some(file).filter(|u| is_web(u)).or(Some(&latest.notes_url).filter(|u| is_web(u))).cloned(),
        Install::System | Install::Homebrew => None,
    };
    Some(Notice { version: latest.version.clone(), build, notes_url: latest.notes_url.clone(), download, install })
}

/// Two version strings name the same version (`v0.2.0` and `0.2.0` do).
fn same_version(a: &str, b: &str) -> bool {
    a == b || matches!((Version::parse(a), Version::parse(b)), (Some(x), Some(y)) if x == y)
}

/// Only web links are opened from the file (never `file:` or a custom scheme).
pub fn is_web(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

// ---- the worker ----

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The plain GET.
fn fetch(url: &str) -> Result<Latest, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .user_agent(format!("Uzumaki/{}", crate::VERSION))
        .build()
        .into();
    let mut resp = agent.get(url).call().map_err(|e| e.to_string())?;
    let body = resp.body_mut().with_config().limit(64 * 1024).read_to_vec().map_err(|e| e.to_string())?;
    let latest: Latest = serde_json::from_slice(&body).map_err(|e| format!("bad latest.json: {e}"))?;
    if Version::parse(&latest.version).is_none() {
        return Err(format!("bad version in latest.json: {:?}", latest.version));
    }
    Ok(latest)
}

fn url() -> String {
    std::env::var("UZUMAKI_UPDATE_URL").ok().filter(|u| !u.is_empty()).unwrap_or_else(|| LATEST_URL.to_string())
}

/// On a worker thread: the check (if `enabled` and due), then `done(record)` with what it
/// knows (with `enabled` off, only when the last check ran, for Settings).
pub fn spawn(enabled: bool, done: impl FnOnce(Record) + Send + 'static) {
    std::thread::Builder::new()
        .name("update-check".into())
        .spawn(move || {
            // No place for the record: nothing to ask, but the caller still hears back.
            let Some(path) = record_path() else { return done(Record::default()) };
            let mut rec = Record::load(&path);
            if enabled && check(&mut rec, now(), || fetch(&url())) {
                rec.save(&path);
            }
            done(rec);
        })
        .ok();
}

/// A manual check on a worker thread: `done(answer, when it ran)`.
pub fn spawn_manual(done: impl FnOnce(Result<Latest, String>, Option<u64>) + Send + 'static) {
    std::thread::Builder::new()
        .name("update-check".into())
        .spawn(move || {
            let path = record_path();
            let mut rec = path.as_deref().map(Record::load).unwrap_or_default();
            let answer = manual(&mut rec, now(), || fetch(&url()));
            if let Some(path) = &path {
                rec.save(path);
            }
            done(answer, rec.last_check);
        })
        .ok();
}

/// Remembers "Skip this version" for this (version, build) (on a worker thread: the UI never
/// waits on the disk).
pub fn skip(version: String, build: u32) {
    std::thread::spawn(move || {
        let Some(path) = record_path() else { return };
        let mut rec = Record::load(&path);
        rec.skipped = Some(version);
        rec.skipped_build = Some(build_of(build));
        rec.save(&path);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `is_newer` for two builds 1.
    fn newer(a: &str, b: &str) -> bool {
        is_newer((a, 1), (b, 1))
    }

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap_or_else(|| panic!("{s}"))
    }

    #[test]
    fn versions_compare_like_semver() {
        assert!(newer("0.2.0", "0.1.0"));
        assert!(newer("0.1.1", "0.1.0"));
        assert!(newer("1.0.0", "0.99.99"));
        assert!(newer("0.10.0", "0.9.0"), "numbers, not text");
        assert!(newer("v0.2.0", "0.1.0"), "a leading v is fine");
        assert!(!newer("0.1.0", "0.1.0"));
        assert!(!newer("0.0.9", "0.1.0"));
        // Pre-releases come before their release.
        assert!(newer("0.2.0", "0.2.0-beta.1"));
        assert!(!newer("0.2.0-beta.1", "0.2.0"));
        assert!(newer("0.2.0-beta.1", "0.1.0"));
        assert!(newer("0.2.0-beta.10", "0.2.0-beta.2"));
        assert!(newer("0.2.0-rc.1", "0.2.0-beta.9"));
        assert!(newer("0.2.0-beta.1.1", "0.2.0-beta.1"));
        assert!(newer("0.2.0-beta", "0.2.0-1"), "words after numbers");
        // Build data doesn't count.
        assert!(!newer("0.1.0+abc", "0.1.0"));
        // Unreadable is never newer.
        for bad in ["", "1", "1.2", "1.2.3.4", "x.y.z", "1.2.-3", "1.2.3-", "1.2.3-a..b", "latest"] {
            assert!(Version::parse(bad).is_none(), "{bad}");
            assert!(!newer(bad, "0.1.0"), "{bad}");
        }
        assert_eq!(v("1.2.3-alpha.1"), Version { major: 1, minor: 2, patch: 3, pre: vec!["alpha".into(), "1".into()] });
    }

    #[test]
    fn a_higher_build_of_the_same_version_is_newer() {
        assert!(is_newer(("0.1.0", 2), ("0.1.0", 1)));
        assert!(!is_newer(("0.1.0", 1), ("0.1.0", 2)), "running a later build");
        assert!(!is_newer(("0.1.0", 1), ("0.1.0", 1)));
        assert!(is_newer(("v0.1.0", 3), ("0.1.0", 2)), "a leading v is the same version");
        // The version wins over the build.
        assert!(is_newer(("0.2.0", 1), ("0.1.0", 9)));
        assert!(!is_newer(("0.1.0", 9), ("0.2.0", 1)));
        // Build 0 (a file without `build`) is build 1.
        assert!(!is_newer(("0.1.0", 0), ("0.1.0", 1)));
        assert!(!is_newer(("0.1.0", 1), ("0.1.0", 0)));
        assert!(is_newer(("0.1.0", 2), ("0.1.0", 0)));
        assert!(!is_newer(("latest", 5), ("0.1.0", 1)), "unreadable is never newer");
    }

    #[test]
    fn this_build_reads_as_a_version() {
        assert!(crate::BUILD >= 1);
        assert!(Version::parse(crate::VERSION).is_some());
    }

    #[test]
    fn the_bridge_has_the_apps_version() {
        let pkg: serde_json::Value = serde_json::from_str(include_str!("../../bridge/package.json")).unwrap();
        assert_eq!(pkg["version"], crate::VERSION, "bridge/package.json and the workspace Cargo.toml disagree");
        assert_eq!(pkg["license"], env!("CARGO_PKG_LICENSE"));
    }

    #[test]
    fn a_manual_check_ignores_the_day_and_counts_as_it() {
        let t0 = 1_800_000_000;
        let latest = Latest { version: "0.2.0".into(), ..Default::default() };
        let mut rec = Record { last_check: Some(t0 - 60), skipped: Some("0.2.0".into()), ..Default::default() };
        let mut fetched = false;
        let answer = manual(&mut rec, t0, || {
            fetched = true;
            Ok(latest.clone())
        });
        assert!(fetched, "checked a minute ago, asked again anyway");
        assert_eq!(answer, Ok(latest.clone()));
        assert_eq!((rec.last_check, rec.latest.as_ref()), (Some(t0), Some(&latest)));
        assert!(!due(rec.last_check, t0 + 10), "the daily check waits a day from it");
        assert_eq!(rec.skipped.as_deref(), Some("0.2.0"), "the skip stays for the daily notice");
        // A failure keeps the last answer and still counts.
        let answer = manual(&mut rec, t0 + 5, || Err("offline".into()));
        assert!(answer.is_err());
        assert_eq!((rec.last_check, rec.latest.as_ref()), (Some(t0 + 5), Some(&latest)));
    }

    #[test]
    fn last_checked_reads_like_a_person_would_say_it() {
        let at = |year, month, day, hour, minute, days| LocalTime { year, month, day, hour, minute, days };
        let now = at(2026, 11, 12, 16, 0, 20_404);
        assert_eq!(when_label(at(2026, 11, 12, 14, 5, 20_404), now), "today at 14:05");
        assert_eq!(when_label(at(2026, 11, 11, 9, 30, 20_403), now), "yesterday at 09:30");
        assert_eq!(when_label(at(2026, 11, 3, 18, 0, 20_395), now), "3 Nov at 18:00");
        assert_eq!(when_label(at(2025, 12, 28, 18, 0, 20_085), now), "28 Dec 2025 at 18:00");
        assert_eq!(last_checked(None), "Not checked yet");
        let t = now_secs_for_test();
        assert!(last_checked(Some(t)).starts_with("Last checked: today at "), "{}", last_checked(Some(t)));
        let lt = local_time(t).unwrap();
        assert!((1..=12).contains(&lt.month) && lt.hour < 24 && lt.minute < 60);
    }

    fn now_secs_for_test() -> u64 {
        now()
    }

    #[test]
    fn checks_at_most_once_a_day() {
        let t0 = 1_800_000_000;
        let latest = Latest { version: "0.2.0".into(), ..Default::default() };
        let mut rec = Record::default();
        let mut fetches = 0;
        assert!(check(&mut rec, t0, || {
            fetches += 1;
            Ok(latest.clone())
        }));
        assert_eq!(rec.last_check, Some(t0));
        assert_eq!(rec.latest.as_ref(), Some(&latest));
        // Later the same day: no fetch, the answer is kept for the notice.
        for later in [t0 + 1, t0 + 3600, t0 + DAY - 1] {
            assert!(!check(&mut rec, later, || {
                fetches += 1;
                Err("must not run".into())
            }));
        }
        assert_eq!(fetches, 1);
        assert_eq!(rec.latest.as_ref(), Some(&latest));
        // A day later it runs again; a failure counts as the day's check and keeps the old answer.
        assert!(check(&mut rec, t0 + DAY, || Err("offline".into())));
        assert_eq!(rec.last_check, Some(t0 + DAY));
        assert_eq!(rec.latest.as_ref(), Some(&latest));
        assert!(!due(rec.last_check, t0 + DAY + 10));
        // The clock went back: check again rather than wait for ever.
        assert!(due(Some(t0), t0 - 5));
        assert!(due(None, 0));
    }

    #[test]
    fn the_timer_checks_when_a_day_has_passed_and_not_before() {
        let t0 = 1_800_000_000;
        let wanted = auto_wanted;
        // Fake clock: the app started and checked at t0, then stays open for days.
        let last = Some(t0);
        let mut ticks_that_fetch = Vec::new();
        let mut last_check = last;
        for hour in 1..=72u64 {
            let now = t0 + hour * RECHECK.as_secs();
            if wanted(true, false, false, last_check, now) {
                ticks_that_fetch.push(hour);
                last_check = Some(now); // the worker's answer (or failure) counts as the day's check
            }
        }
        assert_eq!(ticks_that_fetch, [24, 48, 72], "once a day, on the first tick a day after the last check");
        // Each rule on its own.
        assert!(wanted(true, false, false, last, t0 + DAY));
        assert!(!wanted(true, false, false, last, t0 + DAY - 1), "not yet");
        assert!(!wanted(false, false, false, last, t0 + 9 * DAY), "switched off");
        assert!(!wanted(true, true, false, last, t0 + 9 * DAY), "a notice already shows: no second one");
        assert!(!wanted(true, false, true, last, t0 + 9 * DAY), "a check is already running");
        assert!(wanted(true, false, false, None, t0), "never checked");
        assert!(wanted(true, false, false, last, t0 - 5), "the clock went back");
        // The computer slept for three days: the first tick after waking checks, once.
        assert!(wanted(true, false, false, last, t0 + 3 * DAY + 100));
        assert!(!wanted(true, false, false, Some(t0 + 3 * DAY + 100), t0 + 3 * DAY + 100 + RECHECK.as_secs()));
        assert!(RECHECK < Duration::from_secs(DAY) && RECHECK >= Duration::from_secs(60), "rare, but not once a day");
    }

    #[test]
    fn the_record_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("uzumaki-update-test-{}", std::process::id()));
        let path = dir.join("update.json");
        let rec = Record {
            last_check: Some(42),
            latest: Some(Latest { version: "0.3.0".into(), notes_url: "https://x/n".into(), ..Default::default() }),
            skipped: Some("0.2.0".into()),
            skipped_build: Some(2),
        };
        rec.save(&path);
        assert_eq!(Record::load(&path), rec);
        // Missing or broken: a fresh record (so a check is due).
        std::fs::write(&path, b"{nope").unwrap();
        assert_eq!(Record::load(&path), Record::default());
        assert_eq!(Record::load(&dir.join("missing.json")), Record::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn sample() -> Latest {
        serde_json::from_str(
            r#"{"version":"0.2.0","date":"2026-12-01","notes_url":"https://github.com/hanami117/uzumaki/releases/tag/v0.2.0",
                "downloads":{"macos_dmg":"https://github.com/hanami117/uzumaki/releases/download/v0.2.0/Uzumaki-0.2.0.dmg",
                "linux_tarball":"https://github.com/hanami117/uzumaki/releases/download/v0.2.0/uzumaki-0.2.0.tar.gz",
                "appimage":"https://github.com/hanami117/uzumaki/releases/download/v0.2.0/Uzumaki-0.2.0-x86_64.AppImage",
                "aur":"https://aur.archlinux.org/packages/uzumaki","brew":"brew install --cask hanami117/uzumaki/uzumaki"}}"#,
        )
        .unwrap()
    }

    #[test]
    fn latest_json_parses_and_tolerates_missing_fields() {
        let l = sample();
        assert_eq!(l.version, "0.2.0");
        assert!(l.downloads.macos_dmg.ends_with(".dmg"));
        let bare: Latest = serde_json::from_str(r#"{"version":"0.2.0","extra":1}"#).unwrap();
        assert_eq!(bare.downloads, Downloads::default());
        // A file from before the AppImage: no `appimage`, the rest as it was.
        let old: Latest = serde_json::from_str(r#"{"version":"0.2.0","downloads":{"macos_dmg":"https://x/U.dmg","linux_tarball":"https://x/u.tar.gz"}}"#).unwrap();
        assert_eq!((old.downloads.appimage.as_str(), old.downloads.linux_tarball.as_str()), ("", "https://x/u.tar.gz"));
        // No `build` (every file before builds existed): 0, compared as build 1.
        assert_eq!((l.build, bare.build), (0, 0));
        assert!(notice(&bare, ("0.2.0", 1), None, Install::Direct, true).is_none(), "the same release");
        let built: Latest = serde_json::from_str(r#"{"version":"0.2.0","build":3}"#).unwrap();
        assert_eq!(built.build, 3);
    }

    #[test]
    fn a_rebuild_shows_with_its_build_and_skips_per_build() {
        let l = Latest { build: 2, ..sample() };
        let n = notice(&l, ("0.2.0", 1), None, Install::Direct, true).unwrap();
        assert_eq!((n.version.as_str(), n.build), ("0.2.0", 2));
        assert_eq!(n.message(), "Uzumaki 0.2.0 (build 2) is available.");
        assert!(notice(&l, ("0.2.0", 2), None, Install::Direct, true).is_none(), "already on build 2");
        // Skipping build 1 of 0.2.0 doesn't hide build 2; skipping build 2 does.
        assert!(notice(&l, ("0.1.0", 1), Some(("0.2.0", 1)), Install::Direct, true).is_some());
        assert!(notice(&l, ("0.1.0", 1), Some(("0.2.0", 2)), Install::Direct, true).is_none());
        assert!(notice(&l, ("0.1.0", 1), Some(("v0.2.0", 2)), Install::Direct, true).is_none());
        // A record from before builds: its skip is build 1.
        let old: Record = serde_json::from_str(r#"{"last_check":1,"skipped":"0.2.0"}"#).unwrap();
        assert_eq!(old.skipped(), Some(("0.2.0", 1)));
        assert!(notice(&sample(), ("0.1.0", 1), old.skipped(), Install::Direct, true).is_none());
        assert!(notice(&l, ("0.1.0", 1), old.skipped(), Install::Direct, true).is_some());
    }

    #[test]
    fn skip_hides_only_that_version() {
        let l = sample();
        assert!(notice(&l, ("0.1.0", 1), None, Install::Direct, true).is_some());
        assert!(notice(&l, ("0.1.0", 1), Some(("0.2.0", 1)), Install::Direct, true).is_none());
        let newer = Latest { version: "0.2.1".into(), ..l.clone() };
        assert!(notice(&newer, ("0.1.0", 1), Some(("0.2.0", 1)), Install::Direct, true).is_some(), "a newer one shows again");
        assert!(notice(&l, ("0.2.0", 1), None, Install::Direct, true).is_none(), "already up to date");
        assert!(notice(&l, ("0.3.0", 1), None, Install::Direct, true).is_none(), "running a newer build");
    }

    #[test]
    fn download_goes_to_this_systems_file() {
        let l = sample();
        let mac = notice(&l, ("0.1.0", 1), None, Install::Direct, true).unwrap();
        assert_eq!(mac.download.as_deref(), Some(l.downloads.macos_dmg.as_str()));
        assert_eq!(mac.message(), "Uzumaki 0.2.0 is available.");
        let linux = notice(&l, ("0.1.0", 1), None, Install::Direct, false).unwrap();
        assert_eq!(linux.download.as_deref(), Some(l.downloads.linux_tarball.as_str()));
        let appimage = notice(&l, ("0.1.0", 1), None, Install::AppImage, false).unwrap();
        assert_eq!(appimage.download.as_deref(), Some(l.downloads.appimage.as_str()));
        assert_eq!(appimage.message(), "Uzumaki 0.2.0 is available.");
        // An older latest.json without the AppImage: the release page, not the tarball.
        let no_appimage = Latest { downloads: Downloads { appimage: String::new(), ..l.downloads.clone() }, ..l.clone() };
        assert_eq!(notice(&no_appimage, ("0.1.0", 1), None, Install::AppImage, false).unwrap().download.as_deref(), Some(l.notes_url.as_str()));
        // No file for this system: the release page; nothing usable: no button.
        let no_file = Latest { downloads: Downloads::default(), ..l.clone() };
        assert_eq!(notice(&no_file, ("0.1.0", 1), None, Install::Direct, true).unwrap().download.as_deref(), Some(l.notes_url.as_str()));
        let odd = Latest { notes_url: "javascript:alert(1)".into(), downloads: Downloads { macos_dmg: "file:///etc/passwd".into(), ..Default::default() }, ..l };
        assert_eq!(notice(&odd, ("0.1.0", 1), None, Install::Direct, true).unwrap().download, None);
    }

    #[test]
    fn package_installs_say_so_and_offer_no_download() {
        let l = sample();
        for (install, tail) in [
            (Install::System, " Update with your package manager."),
            (Install::Homebrew, " Update with your package manager (brew upgrade uzumaki)."),
        ] {
            let n = notice(&l, ("0.1.0", 1), None, install, install == Install::Homebrew).unwrap();
            assert_eq!(n.download, None);
            assert_eq!(n.message(), format!("Uzumaki 0.2.0 is available.{tail}"));
            assert!(n.message().contains("Update with your package manager"));
        }
    }

    /// One request on loopback: what the app sends, and that it reads the answer.
    fn serve_once(status: &'static str, body: &'static str) -> (String, std::thread::JoinHandle<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/latest.json", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut req = Vec::new();
            let mut buf = [0u8; 1024];
            while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = conn.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                req.extend_from_slice(&buf[..n]);
            }
            let reply = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            conn.write_all(reply.as_bytes()).unwrap();
            String::from_utf8_lossy(&req).into_owned()
        });
        (url, server)
    }

    #[test]
    fn fetch_is_a_plain_get_with_a_generic_user_agent() {
        let (url, server) = serve_once("200 OK", r#"{"version":"0.2.0","notes_url":"https://example.com/n"}"#);
        let latest = fetch(&url).unwrap();
        assert_eq!(latest.version, "0.2.0");
        let req = server.join().unwrap().to_ascii_lowercase();
        assert!(req.starts_with("get /latest.json http/1.1\r\n"), "{req}");
        assert!(req.contains(&format!("user-agent: uzumaki/{}\r\n", crate::VERSION)), "{req}");
        assert!(!req.contains("cookie"), "{req}");
        // Header names only: host, user-agent, and ureq's accept / accept-encoding.
        let names: Vec<&str> = req.lines().skip(1).filter_map(|l| l.split_once(':').map(|(n, _)| n)).collect();
        for n in &names {
            assert!(["host", "user-agent", "accept", "accept-encoding"].contains(n), "unexpected header {n}: {req}");
        }

        let (url, server) = serve_once("404 Not Found", "nope");
        assert!(fetch(&url).is_err());
        server.join().unwrap();
        let (url, server) = serve_once("200 OK", r#"{"version":"soon"}"#);
        assert!(fetch(&url).is_err(), "an unreadable version is an error");
        server.join().unwrap();
    }

    #[test]
    fn install_kind_from_the_binarys_path() {
        let none = |_: &Path| false;
        let all = |_: &Path| true;
        let linux = |p: &str| install_kind(Path::new(p), true, false, none);
        assert_eq!(linux("/usr/bin/uzumaki"), Install::System);
        assert_eq!(linux("/usr/lib/uzumaki/uzumaki"), Install::System);
        assert_eq!(linux("/usr/local/bin/uzumaki"), Install::Direct, "hand installs go to /usr/local");
        assert_eq!(linux("/home/me/.local/bin/uzumaki"), Install::Direct);
        assert_eq!(linux("/home/me/src/uzumaki/target/release/uzumaki"), Install::Direct);
        assert_eq!(linux("/usrx/bin/uzumaki"), Install::Direct, "whole path parts only");

        let app = Path::new("/Applications/Uzumaki.app/Contents/MacOS/uzumaki");
        assert_eq!(install_kind(app, false, true, all), Install::Homebrew, "bundle + cask receipt");
        assert_eq!(install_kind(app, false, true, none), Install::Direct, "the DMG");
        let receipt_only = |p: &Path| p == Path::new("/opt/homebrew/Caskroom/uzumaki/.metadata");
        assert_eq!(install_kind(app, false, true, receipt_only), Install::Homebrew);
        let other_cask = |p: &Path| p == Path::new("/opt/homebrew/Caskroom/uzumaki-nightly/.metadata");
        assert_eq!(install_kind(app, false, true, other_cask), Install::Direct);
        let dev = Path::new("/Users/me/uzumaki/target/release/uzumaki");
        assert_eq!(install_kind(dev, false, true, all), Install::Direct, "cargo run is never the cask's");
        let in_caskroom = Path::new("/opt/homebrew/Caskroom/uzumaki/0.1.0/Uzumaki.app/Contents/MacOS/uzumaki");
        assert_eq!(install_kind(in_caskroom, false, true, none), Install::Homebrew);
        // The /usr rule is Linux's only.
        assert_eq!(install_kind(Path::new("/usr/bin/uzumaki"), false, true, none), Install::Direct);
    }
}
