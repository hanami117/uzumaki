//! The update check and its notice.
//!
//! Once per UTC day the app runs (at start, or from a timer that wakes once an hour and asks
//! `auto_wanted`), a worker thread asks `updates.getuzumaki.app/latest.json` (`server/update` in
//! this repo) what the newest release is. The request is a plain GET with the User-Agent
//! `Uzumaki/<version>` and four values in the query, nothing else: `v` (this version), `os`
//! (linux, macos), `arch` (x86_64, aarch64) and `ch`, how the copy was installed (`channel`).
//! Two more flags may be added: `w=1` on the copy's first check of the ISO week and `m=1` on its
//! first of the calendar month (both UTC), so the server can count weekly and monthly users without
//! any id; and `repeat=1` on a manual check when the day was already counted. The server keeps
//! only a daily count per (version, os, arch, channel). If it can't be reached, the static
//! `getuzumaki.app/latest.json` is asked instead (nothing is counted there). A failed check is
//! tried again at the next hourly tick.
//!
//! A newer version (semver) that the user hasn't skipped shows a quiet banner (`ui::banners`):
//! "What's new", "Download" (opens the browser), "Skip this version", and ✕ (until the next
//! start). Nothing is ever downloaded by the app. A copy installed by a package manager (built
//! with `UZUMAKI_CHANNEL=aur`, a binary under `/usr`, a Homebrew cask) is told to update with it
//! and gets no Download button. If this version is below the file's `minimum_supported`, the
//! banner is a warning that can't be closed or skipped: this version no longer works with Apple
//! Music. The app keeps running either way.
//!
//! When the last check ran, its answer and the skipped version live in
//! `$XDG_STATE_HOME/uzumaki/update.json` (else `~/.local/state/…`, on macOS too): state, not
//! settings. The switch is `Settings::check_updates` (Settings → General): off, nothing is sent.
//! Headless runs and tests never fetch: only the real app's `State` (with a handle) starts the
//! worker.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Where the latest release is described (the counting Worker). `UZUMAKI_UPDATE_URL` overrides it
/// (testing; then there is no fallback).
pub const LATEST_URL: &str = "https://updates.getuzumaki.app/latest.json";
/// Asked when the Worker can't be (the site's static file, which the Worker passes through).
pub const FALLBACK_URL: &str = "https://getuzumaki.app/latest.json";
/// Seconds in a day: checks go by UTC day (`utc_day`).
pub const DAY: u64 = 86_400;
/// A failed automatic check is tried again after this long (the hourly tick; a few minutes less, so
/// a tick that comes a little early still retries).
pub const RETRY: u64 = 55 * 60;
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
    /// The oldest version that still works with Apple Music (`null` or missing: no limit). Below it
    /// the notice is a warning that can't be closed (`Notice::required`).
    #[serde(default)]
    pub minimum_supported: Option<String>,
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

/// `current` is below `latest`'s `minimum_supported` (versions only, builds ignored). Anything
/// unreadable is never below it.
pub fn below_minimum(latest: &Latest, current: &str) -> bool {
    match (latest.minimum_supported.as_deref().and_then(Version::parse), Version::parse(current)) {
        (Some(min), Some(cur)) => cur < min,
        _ => false,
    }
}

/// `candidate` (version, build) is newer than `current`: a newer version, or the same version
/// with a higher build. Anything unreadable is never newer.
pub fn is_newer(candidate: (&str, u32), current: (&str, u32)) -> bool {
    match (Version::parse(candidate.0), Version::parse(current.0)) {
        (Some(a), Some(b)) => a.cmp(&b).then(build_of(candidate.1).cmp(&build_of(current.1))) == Ordering::Greater,
        _ => false,
    }
}

// ---- once per UTC day ----

/// The UTC day of `t` (days since 1970-01-01).
pub fn utc_day(t: u64) -> u64 {
    t / DAY
}

/// The ISO week of `t` (UTC), named by its Monday's `utc_day`. 1970-01-01 was a Thursday.
pub fn iso_week(t: u64) -> i64 {
    let d = utc_day(t) as i64;
    d - (d + 3).rem_euclid(7)
}

/// The UTC calendar month of `t`: (year, 1–12).
pub fn utc_month(t: u64) -> (i64, u32) {
    // Howard Hinnant's civil_from_days.
    let z = utc_day(t) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month)
}

/// What a check adds to the query besides version, OS, architecture and channel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flags {
    /// `w=1`: the first counted check of this ISO week.
    pub week: bool,
    /// `m=1`: the first counted check of this calendar month.
    pub month: bool,
    /// `repeat=1`: a manual check on a day that was already counted (the server doesn't count it).
    pub repeat: bool,
}

/// The flags for a check at `now` after the last answered one at `last_check`. Only a manual check
/// can be a repeat (the automatic one is never due twice on a day). A clock that went back is a new
/// day, not a repeat.
pub fn flags(last_check: Option<u64>, now: u64, manual: bool) -> Flags {
    let Some(t) = last_check.filter(|&t| t <= now) else {
        return Flags { week: true, month: true, repeat: false };
    };
    if manual && utc_day(t) == utc_day(now) {
        return Flags { repeat: true, ..Flags::default() };
    }
    Flags { week: iso_week(t) != iso_week(now), month: utc_month(t) != utc_month(now), repeat: false }
}

/// The install channels the server knows (`server/update/src/index.js`, `CHANNEL`).
pub const CHANNELS: [&str; 7] = ["dmg", "homebrew", "appimage", "aur", "system", "install-sh", "source"];

/// The query: `v=…&os=…&arch=…&ch=…`, then `&w=1`, `&m=1`, `&repeat=1` as `flags` says.
pub fn query(version: &str, os: &str, arch: &str, channel: &str, flags: Flags) -> String {
    let mut q = format!("v={}&os={}&arch={}&ch={}", enc(version), enc(os), enc(arch), enc(channel));
    for (on, f) in [(flags.week, "&w=1"), (flags.month, "&m=1"), (flags.repeat, "&repeat=1")] {
        if on {
            q.push_str(f);
        }
    }
    q
}

/// `url` with `query` added (after `?`, or `&` when it has one).
pub fn with_query(url: &str, query: &str) -> String {
    format!("{url}{}{query}", if url.contains('?') { '&' } else { '?' })
}

/// Percent-encodes all but URL-safe characters.
fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// What the update check remembers between runs (`update.json`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Record {
    /// Unix seconds of the last answered check.
    pub last_check: Option<u64>,
    /// Unix seconds of the last failed attempt since then (the hourly retry waits from it).
    pub last_attempt: Option<u64>,
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

/// The automatic check is due: no answered check yet this UTC day (or ever, or the clock went
/// back), and no failed attempt in the last `RETRY`.
pub fn due(last_check: Option<u64>, last_attempt: Option<u64>, now: u64) -> bool {
    let new_day = last_check.is_none_or(|t| now < t || utc_day(t) != utc_day(now));
    let retry = last_attempt.is_none_or(|a| now < a || now - a >= RETRY);
    new_day && retry
}

/// Whether the timer should start the automatic check now: the switch is on, no check is running,
/// and `due` says so. A notice showing doesn't hold it back: the day's check still runs (and keeps
/// the notice current).
pub fn auto_wanted(enabled: bool, busy: bool, last_check: Option<u64>, last_attempt: Option<u64>, now: u64) -> bool {
    enabled && !busy && due(last_check, last_attempt, now)
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

/// Keeps `answer` in the record: an answer is the day's check, a failure is an attempt (retried
/// after `RETRY`) and keeps the last answer.
fn record(rec: &mut Record, now: u64, answer: &Result<Latest, String>) {
    match answer {
        Ok(latest) => {
            rec.last_check = Some(now);
            rec.last_attempt = None;
            rec.latest = Some(latest.clone());
        }
        Err(e) => {
            rec.last_attempt = Some(now);
            eprintln!("uzumaki: update check failed: {e}");
        }
    }
}

/// The record after an automatic check at `now`: due → `fetch` runs with the day's flags and its
/// answer is kept; not due → unchanged. Returns whether `fetch` ran.
pub fn check(rec: &mut Record, now: u64, fetch: impl FnOnce(Flags) -> Result<Latest, String>) -> bool {
    if !due(rec.last_check, rec.last_attempt, now) {
        return false;
    }
    let answer = fetch(flags(rec.last_check, now, false));
    record(rec, now, &answer);
    true
}

/// A manual check ("Check for Updates…", Settings → General → Check now): always asks. On a day
/// that already had an answered check it is a `repeat` (not counted again); otherwise its answer is
/// the day's check.
pub fn manual(rec: &mut Record, now: u64, fetch: impl FnOnce(Flags) -> Result<Latest, String>) -> Result<Latest, String> {
    let answer = fetch(flags(rec.last_check, now, true));
    record(rec, now, &answer);
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
    /// The AUR package (built with `UZUMAKI_CHANNEL=aur`).
    Aur,
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

/// The channel a package built this copy for: `UZUMAKI_CHANNEL` at compile time (the AUR's
/// PKGBUILD sets `aur`, install.sh `install-sh`), when it is one of `CHANNELS`.
pub fn build_channel() -> Option<&'static str> {
    option_env!("UZUMAKI_CHANNEL").and_then(|c| CHANNELS.iter().copied().find(|k| *k == c))
}

/// The install kind from the build's channel and what the running copy looks like (`runtime`).
/// The build's channel wins, except `dmg`: the Homebrew cask installs the same DMG, so its receipt
/// still makes it Homebrew.
pub fn resolve_install(build: Option<&str>, runtime: Install) -> Install {
    match build {
        Some("aur") => Install::Aur,
        Some("homebrew") => Install::Homebrew,
        Some("system") => Install::System,
        Some("appimage") => Install::AppImage,
        Some("dmg") if runtime == Install::Homebrew => Install::Homebrew,
        Some("dmg" | "install-sh" | "source") => Install::Direct,
        _ => runtime,
    }
}

/// The `ch` value sent with the check (one of `CHANNELS`).
pub fn channel_of(install: Install, build: Option<&'static str>, mac: bool) -> &'static str {
    match install {
        Install::Aur => "aur",
        Install::Homebrew => "homebrew",
        Install::System => "system",
        Install::AppImage => "appimage",
        Install::Direct => match build {
            Some(c @ ("dmg" | "install-sh" | "source")) => c,
            _ if mac => "dmg",
            _ => "source",
        },
    }
}

/// This copy's install kind. An AppImage's binary runs from a temporary mount, so the
/// runtime's `APPIMAGE` variable (the .AppImage file's path) tells it.
pub fn this_install() -> Install {
    let runtime = if cfg!(target_os = "linux") && std::env::var_os("APPIMAGE").is_some_and(|v| !v.is_empty()) {
        Install::AppImage
    } else {
        let exe = std::env::current_exe().and_then(|p| p.canonicalize()).unwrap_or_default();
        install_kind(&exe, cfg!(target_os = "linux"), cfg!(target_os = "macos"), |p| p.is_dir())
    };
    resolve_install(build_channel(), runtime)
}

/// This copy's `ch` value.
pub fn this_channel() -> &'static str {
    channel_of(this_install(), build_channel(), cfg!(target_os = "macos"))
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
    /// This version is below `minimum_supported`: a warning that can't be closed or skipped.
    pub required: bool,
}

impl Notice {
    /// The banner's line.
    pub fn message(&self) -> String {
        let name = match self.build {
            0 | 1 => format!("Uzumaki {}", self.version),
            b => format!("Uzumaki {} (build {b})", self.version),
        };
        let head = if self.required {
            format!("This version of Uzumaki no longer works with Apple Music. Please update to {name}.")
        } else {
            format!("{name} is available.")
        };
        match self.install {
            Install::Direct | Install::AppImage => head,
            Install::System => format!("{head} Update with your package manager."),
            Install::Aur => format!("{head} Update with your package manager: yay -Syu uzumaki"),
            Install::Homebrew => format!("{head} Update with your package manager: brew update && brew upgrade --cask uzumaki"),
        }
    }
}

/// The notice for `latest`, if it is newer than `current` (version, build) and not skipped.
/// A skip holds for one (version, build): a higher build of the same version shows again. Below
/// `minimum_supported` the notice is `required` and shows even when skipped.
pub fn notice(latest: &Latest, current: (&str, u32), skipped: Option<(&str, u32)>, install: Install, mac: bool) -> Option<Notice> {
    let build = build_of(latest.build);
    let required = below_minimum(latest, current.0);
    let is_skipped = skipped.is_some_and(|(v, b)| same_version(v, &latest.version) && build_of(b) == build);
    if !is_newer((&latest.version, build), current) || (is_skipped && !required) {
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
        Install::System | Install::Homebrew | Install::Aur => None,
    };
    Some(Notice { version: latest.version.clone(), build, notes_url: latest.notes_url.clone(), download, install, required })
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

/// Asks `primary`; if that fails and there is a `fallback`, asks it once instead.
fn fetch_or_fallback(primary: &str, fallback: Option<&str>, get: impl Fn(&str) -> Result<Latest, String>) -> Result<Latest, String> {
    get(primary).or_else(|e| match fallback {
        Some(f) => {
            eprintln!("uzumaki: update server failed ({e}), asking {f}");
            get(f)
        }
        None => Err(e),
    })
}

/// The check with this copy's query: the Worker, else the static file. `UZUMAKI_UPDATE_URL`
/// replaces the Worker (and drops the fallback).
fn fetch_latest(flags: Flags) -> Result<Latest, String> {
    let q = query(crate::VERSION, std::env::consts::OS, std::env::consts::ARCH, this_channel(), flags);
    match std::env::var("UZUMAKI_UPDATE_URL").ok().filter(|u| !u.is_empty()) {
        Some(url) => fetch(&with_query(&url, &q)),
        None => fetch_or_fallback(&with_query(LATEST_URL, &q), Some(FALLBACK_URL), fetch),
    }
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
            if enabled && check(&mut rec, now(), fetch_latest) {
                rec.save(&path);
            }
            done(rec);
        })
        .ok();
}

/// A manual check on a worker thread: `done(answer, the record after it)`.
pub fn spawn_manual(done: impl FnOnce(Result<Latest, String>, Record) + Send + 'static) {
    std::thread::Builder::new()
        .name("update-check".into())
        .spawn(move || {
            let path = record_path();
            let mut rec = path.as_deref().map(Record::load).unwrap_or_default();
            let answer = manual(&mut rec, now(), fetch_latest);
            if let Some(path) = &path {
                rec.save(path);
            }
            done(answer, rec);
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
    fn a_manual_check_always_asks_and_is_a_repeat_on_a_counted_day() {
        let t0 = utc(2026, 11, 12, 10, 0);
        let latest = Latest { version: "0.2.0".into(), ..Default::default() };
        let mut rec = Record { last_check: Some(t0 - 60), skipped: Some("0.2.0".into()), ..Default::default() };
        let mut sent = None;
        let answer = manual(&mut rec, t0, |f| {
            sent = Some(f);
            Ok(latest.clone())
        });
        assert_eq!(sent, Some(Flags { repeat: true, ..Flags::default() }), "checked a minute ago: asked again, not counted again");
        assert_eq!(answer, Ok(latest.clone()));
        assert_eq!((rec.last_check, rec.latest.as_ref()), (Some(t0), Some(&latest)));
        assert!(!due(rec.last_check, rec.last_attempt, t0 + 10), "the automatic check waits for the next UTC day");
        assert_eq!(rec.skipped.as_deref(), Some("0.2.0"), "the skip stays for the daily notice");
        // A failure keeps the last answer and the day's check, and is only an attempt.
        let answer = manual(&mut rec, t0 + 5, |_| Err("offline".into()));
        assert!(answer.is_err());
        assert_eq!((rec.last_check, rec.last_attempt, rec.latest.as_ref()), (Some(t0), Some(t0 + 5), Some(&latest)));
        // The first check of a new day by hand is the day's counted check (and the week's here).
        let monday = utc(2026, 11, 16, 8, 0);
        manual(&mut rec, monday, |f| {
            sent = Some(f);
            Ok(latest.clone())
        })
        .unwrap();
        assert_eq!(sent, Some(Flags { week: true, month: false, repeat: false }));
        assert_eq!((rec.last_check, rec.last_attempt), (Some(monday), None));
        // Never checked: counted, first of the week and of the month.
        let mut fresh = Record::default();
        manual(&mut fresh, t0, |f| {
            sent = Some(f);
            Err("offline".into())
        })
        .unwrap_err();
        assert_eq!(sent, Some(Flags { week: true, month: true, repeat: false }));
        assert_eq!((fresh.last_check, fresh.last_attempt), (None, Some(t0)), "a failure is no day's check");
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
    fn checks_once_per_utc_day_and_retries_a_failure_hourly() {
        let t0 = utc(2026, 11, 12, 9, 0);
        let latest = Latest { version: "0.2.0".into(), ..Default::default() };
        let mut rec = Record::default();
        let mut fetches = 0;
        assert!(check(&mut rec, t0, |_| {
            fetches += 1;
            Ok(latest.clone())
        }));
        assert_eq!(rec.last_check, Some(t0));
        assert_eq!(rec.latest.as_ref(), Some(&latest));
        // Later the same UTC day: no fetch, the answer is kept for the notice.
        for later in [t0 + 1, t0 + 3600, utc(2026, 11, 12, 23, 59)] {
            assert!(!check(&mut rec, later, |_| {
                fetches += 1;
                Err("must not run".into())
            }));
        }
        assert_eq!(fetches, 1);
        // 23:59 → 00:01 UTC is a new day, though only two minutes passed.
        let late = utc(2026, 11, 12, 23, 59);
        let rec2 = Record { last_check: Some(late), ..Default::default() };
        assert!(due(rec2.last_check, None, utc(2026, 11, 13, 0, 1)));
        assert!(!due(rec2.last_check, None, late + 30));
        // The next day it runs; a failure keeps the old answer and the old day, and is retried an
        // hour later (the next tick), not a minute later.
        let next = utc(2026, 11, 13, 9, 0);
        assert!(check(&mut rec, next, |_| Err("offline".into())));
        assert_eq!((rec.last_check, rec.last_attempt), (Some(t0), Some(next)));
        assert_eq!(rec.latest.as_ref(), Some(&latest));
        assert!(!due(rec.last_check, rec.last_attempt, next + 60));
        assert!(due(rec.last_check, rec.last_attempt, next + RECHECK.as_secs()), "the next hourly tick retries");
        assert!(check(&mut rec, next + 3600, |_| Ok(latest.clone())));
        assert_eq!((rec.last_check, rec.last_attempt), (Some(next + 3600), None));
        assert!(!due(rec.last_check, rec.last_attempt, next + 7200));
        // The clock went back: check again rather than wait for ever.
        assert!(due(Some(t0), None, t0 - 5));
        assert!(due(Some(t0), Some(t0), t0 - 5));
        assert!(due(None, None, 0));
    }

    /// Unix seconds of a UTC date and time.
    fn utc(year: i64, month: u32, day: u32, hour: u64, minute: u64) -> u64 {
        // Inverse of utc_month's civil_from_days (days_from_civil).
        let y = if month <= 2 { year - 1 } else { year };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let mp = (month as i64 + 9) % 12;
        let doy = (153 * mp + 2) / 5 + day as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = (era * 146_097 + doe - 719_468) as u64;
        days * DAY + hour * 3600 + minute * 60
    }

    #[test]
    fn utc_days_weeks_and_months() {
        assert_eq!(utc(1970, 1, 1, 0, 0), 0);
        assert_eq!(utc(2026, 11, 12, 0, 0), 1_794_441_600);
        assert_eq!(utc_month(utc(2026, 11, 12, 13, 0)), (2026, 11));
        assert_eq!(utc_month(utc(2024, 2, 29, 23, 59)), (2024, 2), "a leap day");
        assert_eq!(utc_month(utc(2024, 3, 1, 0, 0)), (2024, 3));
        assert_eq!(utc_month(utc(2026, 12, 31, 23, 59)), (2026, 12));
        assert_eq!(utc_month(utc(2027, 1, 1, 0, 0)), (2027, 1));
        assert_eq!(utc_month(0), (1970, 1));
        // ISO weeks start on Monday: 2026-11-09 (Mon) to 2026-11-15 (Sun).
        let week = iso_week(utc(2026, 11, 9, 0, 0));
        assert_eq!(week, utc_day(utc(2026, 11, 9, 0, 0)) as i64, "named by its Monday");
        for d in 9..=15 {
            assert_eq!(iso_week(utc(2026, 11, d, 12, 0)), week, "Nov {d}");
        }
        assert_ne!(iso_week(utc(2026, 11, 16, 0, 0)), week);
        assert_ne!(iso_week(utc(2026, 11, 8, 23, 59)), week);
        // A week across the new year is one week (Mon 2026-12-28 to Sun 2027-01-03).
        assert_eq!(iso_week(utc(2026, 12, 28, 0, 0)), iso_week(utc(2027, 1, 3, 23, 59)));
        assert_eq!(iso_week(0), -3, "1970-01-01 was a Thursday");
    }

    #[test]
    fn week_and_month_flags_go_on_the_first_counted_check_only() {
        let wed = utc(2026, 11, 11, 20, 0);
        let f = |last: Option<u64>, now: u64| flags(last, now, false);
        assert_eq!(f(None, wed), Flags { week: true, month: true, repeat: false }, "never checked");
        assert_eq!(f(Some(utc(2026, 11, 10, 8, 0)), wed), Flags::default(), "same week and month");
        assert_eq!(f(Some(utc(2026, 11, 8, 23, 59)), utc(2026, 11, 9, 0, 1)), Flags { week: true, ..Flags::default() }, "Sunday → Monday");
        assert_eq!(f(Some(utc(2026, 10, 31, 12, 0)), utc(2026, 11, 1, 12, 0)), Flags { week: false, month: true, repeat: false }, "Sat Oct 31 → Sun Nov 1");
        assert_eq!(f(Some(utc(2026, 12, 31, 12, 0)), utc(2027, 1, 1, 12, 0)), Flags { week: false, month: true, repeat: false }, "new year, same ISO week");
        assert_eq!(f(Some(utc(2026, 11, 30, 23, 59)), utc(2026, 12, 7, 0, 0)), Flags { week: true, month: true, repeat: false });
        // Manual: a repeat on a day already counted (no week/month then), else like the automatic one.
        assert_eq!(flags(Some(wed - 3600), wed, true), Flags { repeat: true, ..Flags::default() });
        assert_eq!(flags(Some(utc(2026, 11, 8, 12, 0)), wed, true), Flags { week: true, ..Flags::default() });
        // The clock went back: a fresh start, never a repeat.
        assert_eq!(flags(Some(wed + 60), wed, true), Flags { week: true, month: true, repeat: false });
    }

    #[test]
    fn the_query_carries_only_version_os_arch_channel_and_flags() {
        let q = query("0.2.0", "linux", "x86_64", "aur", Flags::default());
        assert_eq!(q, "v=0.2.0&os=linux&arch=x86_64&ch=aur");
        assert_eq!(query("0.2.0-beta.1", "macos", "aarch64", "dmg", Flags { week: true, month: true, repeat: false }), "v=0.2.0-beta.1&os=macos&arch=aarch64&ch=dmg&w=1&m=1");
        assert_eq!(query("0.2.0", "linux", "x86_64", "source", Flags { repeat: true, ..Flags::default() }), "v=0.2.0&os=linux&arch=x86_64&ch=source&repeat=1");
        assert_eq!(query("1.0.0+x y", "linux", "x86_64", "a&b=c", Flags::default()), "v=1.0.0%2Bx%20y&os=linux&arch=x86_64&ch=a%26b%3Dc", "nothing can add a parameter");
        assert_eq!(with_query(LATEST_URL, "v=1"), "https://updates.getuzumaki.app/latest.json?v=1");
        assert_eq!(with_query("http://127.0.0.1:9/x?a=b", "v=1"), "http://127.0.0.1:9/x?a=b&v=1");
        // This build's values are ones the server counts.
        let mine = query(crate::VERSION, std::env::consts::OS, std::env::consts::ARCH, this_channel(), Flags::default());
        let keys: Vec<&str> = mine.split('&').map(|kv| kv.split_once('=').unwrap().0).collect();
        assert_eq!(keys, ["v", "os", "arch", "ch"]);
        assert!(["linux", "macos", "windows"].contains(&std::env::consts::OS));
        assert!(["x86_64", "aarch64"].contains(&std::env::consts::ARCH));
        assert!(CHANNELS.contains(&this_channel()));
    }

    #[test]
    fn channel_from_the_build_and_the_running_copy() {
        use Install::*;
        // No channel built in: what the copy looks like.
        for i in [Direct, System, Homebrew, AppImage] {
            assert_eq!(resolve_install(None, i), i);
        }
        assert_eq!(channel_of(Direct, None, true), "dmg");
        assert_eq!(channel_of(Direct, None, false), "source");
        assert_eq!(channel_of(System, None, false), "system");
        assert_eq!(channel_of(AppImage, None, false), "appimage");
        assert_eq!(channel_of(Homebrew, None, true), "homebrew");
        // The PKGBUILD's aur wins over the /usr rule.
        assert_eq!(resolve_install(Some("aur"), System), Aur);
        assert_eq!(channel_of(Aur, Some("aur"), false), "aur");
        // install.sh: a hand install, counted as such.
        assert_eq!(resolve_install(Some("install-sh"), Direct), Direct);
        assert_eq!(channel_of(Direct, Some("install-sh"), false), "install-sh");
        // The DMG's build is also the cask's: the receipt still makes it Homebrew.
        assert_eq!(resolve_install(Some("dmg"), Homebrew), Homebrew);
        assert_eq!(resolve_install(Some("dmg"), Direct), Direct);
        assert_eq!(channel_of(Direct, Some("dmg"), true), "dmg");
        assert_eq!(resolve_install(Some("appimage"), Direct), AppImage);
        assert_eq!(resolve_install(Some("homebrew"), Direct), Homebrew);
        // Every answer is one the server knows.
        for i in [Direct, System, Homebrew, AppImage, Aur] {
            for b in [None, Some("dmg"), Some("install-sh"), Some("source"), Some("aur")] {
                for mac in [true, false] {
                    assert!(CHANNELS.contains(&channel_of(i, b, mac)));
                }
            }
        }
    }

    #[test]
    fn the_timer_checks_once_each_utc_day() {
        let t0 = utc(2026, 11, 12, 18, 30);
        let wanted = auto_wanted;
        // Fake clock: the app started and checked at t0, then stays open for days.
        let mut ticks_that_fetch = Vec::new();
        let mut last_check = Some(t0);
        for hour in 1..=72u64 {
            let now = t0 + hour * RECHECK.as_secs();
            if wanted(true, false, last_check, None, now) {
                ticks_that_fetch.push(hour);
                last_check = Some(now);
            }
        }
        // 18:30 + 6 h = 00:30 the next day, then every 24 h.
        assert_eq!(ticks_that_fetch, [6, 30, 54], "the first tick after each UTC midnight");
        // Each rule on its own.
        let last = Some(t0);
        let tomorrow = utc(2026, 11, 13, 0, 5);
        assert!(wanted(true, false, last, None, tomorrow));
        assert!(!wanted(true, false, last, None, utc(2026, 11, 12, 23, 59)), "not yet");
        assert!(!wanted(false, false, last, None, tomorrow), "switched off");
        assert!(!wanted(true, true, last, None, tomorrow), "a check is already running");
        assert!(!wanted(true, false, last, Some(tomorrow - 60), tomorrow), "failed a minute ago");
        assert!(wanted(true, false, last, Some(tomorrow - 3600), tomorrow), "failed an hour ago: retry");
        assert!(wanted(true, false, None, None, t0), "never checked");
        assert!(wanted(true, false, last, None, t0 - 5), "the clock went back");
        // The computer slept for three days: the first tick after waking checks, once.
        let woke = t0 + 3 * DAY + 100;
        assert!(wanted(true, false, last, None, woke));
        assert!(!wanted(true, false, Some(woke), None, woke + RECHECK.as_secs()));
        assert!(RECHECK < Duration::from_secs(DAY) && RECHECK >= Duration::from_secs(RETRY), "a failed check is retried by the next tick");
    }

    #[test]
    fn the_record_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("uzumaki-update-test-{}", std::process::id()));
        let path = dir.join("update.json");
        let rec = Record {
            last_check: Some(42),
            last_attempt: Some(43),
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
            (Install::Aur, " Update with your package manager: yay -Syu uzumaki"),
            (Install::Homebrew, " Update with your package manager: brew update && brew upgrade --cask uzumaki"),
        ] {
            let n = notice(&l, ("0.1.0", 1), None, install, install == Install::Homebrew).unwrap();
            assert_eq!(n.download, None);
            assert!(!n.required);
            assert_eq!(n.message(), format!("Uzumaki 0.2.0 is available.{tail}"));
            assert!(n.message().contains("Update with your package manager"));
        }
    }

    #[test]
    fn below_minimum_supported_the_notice_cannot_be_skipped() {
        let l = Latest { minimum_supported: Some("0.1.5".into()), ..sample() };
        assert!(below_minimum(&l, "0.1.0"));
        assert!(below_minimum(&l, "0.1.5-beta.1"), "a pre-release of the minimum is below it");
        assert!(!below_minimum(&l, "0.1.5"));
        assert!(!below_minimum(&l, "0.1.9"));
        assert!(!below_minimum(&sample(), "0.0.1"), "no minimum");
        assert!(!below_minimum(&Latest { minimum_supported: Some("soon".into()), ..sample() }, "0.0.1"), "unreadable: no minimum");
        let n = notice(&l, ("0.1.0", 1), Some(("0.2.0", 1)), Install::Direct, true).expect("shows even when skipped");
        assert!(n.required);
        assert_eq!(n.message(), "This version of Uzumaki no longer works with Apple Music. Please update to Uzumaki 0.2.0.");
        assert_eq!(n.download.as_deref(), Some(l.downloads.macos_dmg.as_str()));
        let aur = notice(&l, ("0.1.0", 1), None, Install::Aur, false).unwrap();
        assert!(aur.required && aur.download.is_none());
        assert!(aur.message().ends_with("Please update to Uzumaki 0.2.0. Update with your package manager: yay -Syu uzumaki"), "{}", aur.message());
        // At or above the minimum it is the usual notice (skippable).
        let ok = notice(&l, ("0.1.6", 1), None, Install::Direct, true).unwrap();
        assert!(!ok.required);
        assert!(notice(&l, ("0.1.6", 1), Some(("0.2.0", 1)), Install::Direct, true).is_none());
        // A broken file (minimum above the latest version) shows nothing it couldn't back up.
        let broken = Latest { minimum_supported: Some("9.0.0".into()), ..sample() };
        assert!(notice(&broken, ("0.2.0", 1), None, Install::Direct, true).is_none());
        // The field: missing, null or a version.
        let none: Latest = serde_json::from_str(r#"{"version":"0.2.0"}"#).unwrap();
        let null: Latest = serde_json::from_str(r#"{"version":"0.2.0","minimum_supported":null}"#).unwrap();
        let set: Latest = serde_json::from_str(r#"{"version":"0.2.0","minimum_supported":"0.1.0"}"#).unwrap();
        assert_eq!((none.minimum_supported, null.minimum_supported, set.minimum_supported.as_deref()), (None, None, Some("0.1.0")));
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
    fn the_query_reaches_the_server_and_a_dead_server_falls_back() {
        let (url, server) = serve_once("200 OK", r#"{"version":"0.2.0","minimum_supported":"0.1.0"}"#);
        let q = query("0.1.0", "linux", "x86_64", "aur", Flags { week: true, ..Flags::default() });
        let latest = fetch(&with_query(&url, &q)).unwrap();
        assert_eq!(latest.minimum_supported.as_deref(), Some("0.1.0"));
        let req = server.join().unwrap();
        assert!(req.starts_with("GET /latest.json?v=0.1.0&os=linux&arch=x86_64&ch=aur&w=1 HTTP/1.1\r\n"), "{req}");
        // The Worker failing (5xx, nothing listening): the static file once.
        let (fallback, server) = serve_once("200 OK", r#"{"version":"0.2.0"}"#);
        let dead = "http://127.0.0.1:1/latest.json?v=0.1.0";
        let got = fetch_or_fallback(dead, Some(&fallback), fetch).unwrap();
        assert_eq!(got.version, "0.2.0");
        let req = server.join().unwrap();
        assert!(req.starts_with("GET /latest.json HTTP/1.1\r\n"), "the fallback gets no query: {req}");
        let (bad, server) = serve_once("502 Bad Gateway", "{}");
        let (fallback, server2) = serve_once("200 OK", r#"{"version":"0.3.0"}"#);
        assert_eq!(fetch_or_fallback(&bad, Some(&fallback), fetch).unwrap().version, "0.3.0");
        server.join().unwrap();
        server2.join().unwrap();
        // Without a fallback (UZUMAKI_UPDATE_URL) the error stands; with one, the first answer wins.
        assert!(fetch_or_fallback(dead, None, fetch).is_err());
        let asked = std::cell::RefCell::new(Vec::new());
        let ok = fetch_or_fallback("a", Some("b"), |u| {
            asked.borrow_mut().push(u.to_string());
            Ok(Latest { version: "1.0.0".into(), ..Default::default() })
        });
        assert!(ok.is_ok());
        assert_eq!(*asked.borrow(), ["a"]);
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
