//! The `cliairplay` helper (Music Assistant's AirPlay sender, GPL-3.0): its command line, its
//! `[STATUS]` lines (DESIGN.md §12 of airplay-cli) and what its errors mean for the user.

use crate::discover::{Device, txt_arg};
use std::path::{Path, PathBuf};

/// What the session needs from a status line. Everything else is only logged.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Route(String),
    Connected,
    ClockReady { mode: String, state: String },
    Buffered { ms: u64 },
    Started { requested_ms: u64, at_ms: u64 },
    /// `ACTION=FLUSH` was taken: the helper waits for new audio and the next START.
    Flushed,
    /// The receiver's queue depth (native AirPlay 2): how late a seek is heard.
    Latency { warm_lead_ms: u64 },
    Error { code: String, http: u64, detail: String },
    Eof,
    IdleTimeout,
    Stopped,
}

/// The value of `key=` in a status line (up to the next space, or a double-quoted string).
fn field<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let mut from = 0;
    while let Some(i) = s[from..].find(key) {
        let at = from + i;
        let after = at + key.len();
        if (at == 0 || s.as_bytes()[at - 1] == b' ') && s[after..].starts_with('=') {
            let v = &s[after + 1..];
            if let Some(q) = v.strip_prefix('"') {
                return Some(q.split('"').next().unwrap_or(""));
            }
            return Some(v.split(' ').next().unwrap_or(""));
        }
        from = after;
    }
    None
}

pub fn parse_status(line: &str) -> Option<Status> {
    let rest = &line[line.find("[STATUS] ")? + "[STATUS] ".len()..];
    let word = rest.split(' ').next().unwrap_or("");
    let num = |k: &str| field(rest, k).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    Some(match word {
        "route" => Status::Route(rest["route".len()..].trim().to_string()),
        "connected" => Status::Connected,
        "clock_ready" => Status::ClockReady {
            mode: field(rest, "mode").unwrap_or("").into(),
            state: field(rest, "state").unwrap_or("").into(),
        },
        "audio" => Status::Buffered { ms: num("buffered_ms") },
        "started" => Status::Started { requested_ms: num("requested_unix_ms"), at_ms: num("at_unix_ms") },
        "flushed" => Status::Flushed,
        "latency" => Status::Latency { warm_lead_ms: num("warm_lead_ms") },
        "error" => Status::Error {
            code: field(rest, "code").unwrap_or("").into(),
            http: num("http"),
            detail: field(rest, "detail").unwrap_or("").into(),
        },
        "eof" => Status::Eof,
        "idle_timeout" => Status::IdleTimeout,
        "stopped" => Status::Stopped,
        _ => return None,
    })
}

/// A plain-words line for an error code, for the log.
pub fn explain(code: &str) -> &'static str {
    match code {
        "auth_required" => {
            "the speaker wants a password or a pairing. HomePod: Home app → Home Settings → Speakers & TV → \
             Allow Speaker & TV Access → \"Everyone\" or \"Anyone on the Same Network\" (no password). \
             Apple TV: pair with the PIN it shows"
        }
        "auth_failed" => "the speaker rejected the password or the stored pairing",
        "connect_failed" => "could not set up the session (speaker off or asleep? another Wi-Fi? firewall?)",
        "start_failed" => "the speaker accepted the session but would not schedule the start",
        "flush_failed" => "the speaker did not take the flush (a seek is heard a little later)",
        _ => "see the cliairplay lines above",
    }
}

pub const STALLED_HINT: &str = "the speaker never answered our clock (UDP 319/320): it would play silence. \
     Check the firewall (allow incoming connections for cliairplay; Linux: UDP 319 and 320)";

/// The settings that shape the command line.
#[derive(Clone, Debug)]
pub struct Options {
    pub volume: u8,
    pub debug: Option<u8>,
    pub extra: Vec<String>,
    pub dacp: String,
    pub active_remote: u32,
    /// The device's AirPlay password (`pw=true` devices).
    pub password: Option<String>,
    /// Stored HomeKit pairing (from `--pair-setup`, with the same `dacp`).
    pub auth: Option<String>,
}

impl Options {
    /// Options with a DACP id and remote id made from `seed` (the DACP id is the pairing's
    /// identity: keep the seed to keep a pairing).
    pub fn seeded(seed: u64, volume: u8) -> Self {
        Options {
            volume,
            debug: None,
            extra: Vec::new(),
            dacp: format!("{seed:016X}"),
            active_remote: (seed >> 16) as u32 & 0x7fff_ffff,
            password: None,
            auth: None,
        }
    }
}

/// The command line for one speaker, after Music Assistant's `_build_cli_args`: `--protocol auto`
/// picks RAOP or AirPlay 2 (native or compat, PTP or NTP) from the TXT records. PCM is s16le stereo
/// 44.1 kHz on stdin (cliairplay's default, said explicitly).
pub fn args(device: &Device, cmdpipe: &Path, opt: &Options) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "--protocol".into(),
        "auto".into(),
        "--dacp".into(),
        opt.dacp.clone(),
        "--activeremote".into(),
        opt.active_remote.to_string(),
        "--cmdpipe".into(),
        cmdpipe.display().to_string(),
        "--samplerate".into(),
        "44100".into(),
        "--bitdepth".into(),
        "16".into(),
        "--channels".into(),
        "2".into(),
        "--volume".into(),
        opt.volume.min(100).to_string(),
    ];
    if let Some(ap) = &device.airplay {
        a.extend(["--port".into(), ap.port.to_string(), "--name".into(), device.name.clone(), "--hostname".into(), device.hostname.clone()]);
    } else if let Some(r) = &device.raop {
        a.extend(["--port".into(), r.port.to_string()]);
    }
    if let Some(r) = &device.raop {
        a.extend(["--udn".into(), r.instance.clone()]);
        for k in ["et", "am", "pk", "pw", "cn"] {
            if let Some(v) = r.get(k).filter(|v| !v.is_empty()) {
                a.extend([format!("--{k}"), v.to_string()]);
            }
        }
        if let Some(md) = r.get("md") {
            a.extend(["--md".into(), md.to_string()]);
        }
    }
    let txt = txt_arg(device);
    if !txt.is_empty() {
        a.extend(["--txt".into(), txt]);
    }
    if let Some(p) = opt.password.as_ref().filter(|p| !p.is_empty()) {
        a.extend(["--password".into(), p.clone()]);
    }
    if let Some(c) = opt.auth.as_ref().filter(|c| !c.is_empty()) {
        a.extend(["--auth".into(), c.clone()]);
    }
    if let Some(d) = opt.debug {
        a.extend(["--debug".into(), d.to_string()]);
    }
    a.extend(opt.extra.iter().cloned());
    a.push(device.ip.to_string());
    a
}

/// The command line of a HomeKit pairing (`--pair-setup`): the device shows a PIN, the helper
/// reads it on stdin and prints `CREDENTIALS: <hex>`. Needs an IPv4 address.
pub fn pair_args(device: &Device, dacp: &str) -> Vec<String> {
    let port = device.airplay.as_ref().map_or(7000, |s| s.port);
    vec!["--pair-setup".into(), "--port".into(), port.to_string(), "--dacp".into(), dacp.into(), device.ip.to_string()]
}

/// The credentials in a `--pair-setup` output line.
pub fn parse_credentials(line: &str) -> Option<&str> {
    let c = line.trim().strip_prefix("CREDENTIALS:")?.trim();
    (!c.is_empty() && c.chars().all(|ch| ch.is_ascii_hexdigit())).then_some(c)
}

/// Where the helper is: `$UZUMAKI_CLIAIRPLAY`; next to the program (`cliairplay`); in a macOS
/// bundle's Resources; an installed copy (`<prefix>/share/uzumaki/cliairplay` for install.sh,
/// `<prefix>/lib/uzumaki/cliairplay` for packages); the source tree's `target/cliairplay/`
/// (scripts/airplay/build-cliairplay.sh); else `cliairplay` on the PATH.
pub fn find(env_path: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(p) = env_path {
        return p.is_file().then_some(p);
    }
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join("cliairplay"));
        candidates.push(dir.join("../Resources/cliairplay"));
        candidates.push(dir.join("../share/uzumaki/cliairplay"));
        candidates.push(dir.join("../lib/uzumaki/cliairplay"));
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/cliairplay/cliairplay"));
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|d| d.join("cliairplay")));
    }
    candidates.into_iter().find(|p| p.is_file()).map(|p| p.canonicalize().unwrap_or(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::homepod;

    #[test]
    fn status_lines_parse() {
        assert_eq!(parse_status("[STATUS] connected"), Some(Status::Connected));
        assert_eq!(parse_status("[12:00:00.123] x:1 [STATUS] audio buffered_ms=93"), Some(Status::Buffered { ms: 93 }));
        assert_eq!(
            parse_status("[STATUS] started requested_unix_ms=0 at_unix_ms=1763100000250 head_unix_ms=5"),
            Some(Status::Started { requested_ms: 0, at_ms: 1763100000250 })
        );
        assert_eq!(
            parse_status(r#"[STATUS] error code=auth_required http=401 detail="pair-setup refused by device""#),
            Some(Status::Error { code: "auth_required".into(), http: 401, detail: "pair-setup refused by device".into() })
        );
        assert_eq!(
            parse_status("[STATUS] clock_ready mode=ptp state=stalled streak_ms=0 exchanges=0 ready_in_ms=0 ready_at_unix_ms=0"),
            Some(Status::ClockReady { mode: "ptp".into(), state: "stalled".into() })
        );
        assert_eq!(parse_status("[STATUS] route protocol=airplay2 flow=native timing=ptp buffered=0"), Some(Status::Route("protocol=airplay2 flow=native timing=ptp buffered=0".into())));
        assert_eq!(parse_status("[STATUS] eof"), Some(Status::Eof));
        assert_eq!(parse_status("[STATUS] latency warm_lead_ms=600 render_lead_ms=250"), Some(Status::Latency { warm_lead_ms: 600 }));
        assert_eq!(parse_status("[STATUS] flushed head_unix_ms=5"), Some(Status::Flushed));
        assert_eq!(parse_status("[STATUS] capabilities requested=0x0"), None);
        assert_eq!(parse_status("plain log line"), None);
    }

    #[test]
    fn field_needs_a_whole_key() {
        assert_eq!(field("xhttp=1 http=2", "http"), Some("2"));
        assert_eq!(field("a=1", "b"), None);
    }

    #[test]
    fn homepod_command_line() {
        let opt = Options { extra: vec!["--timing".into(), "ntp".into()], dacp: "0123456789ABCDEF".into(), active_remote: 42, ..Options::seeded(0, 30) };
        let a = args(&homepod(), Path::new("/tmp/x/cmd.fifo"), &opt);
        let s = a.join(" ");
        assert!(s.starts_with("--protocol auto --dacp 0123456789ABCDEF --activeremote 42 --cmdpipe /tmp/x/cmd.fifo"), "{s}");
        assert!(s.contains("--volume 30 --port 7000 --name Bedroom --hostname Bedroom.local."), "{s}");
        assert!(s.contains("--udn AABBCCDDEEFF@Bedroom --et 0,3,5 --am AudioAccessory5,1 --cn 0,1,2,3"), "{s}");
        assert!(s.contains("--timing ntp"), "{s}");
        // The TXT blob is one argument; the device address is last.
        assert!(a.iter().any(|x| x.starts_with("acl=0 features=")));
        assert_eq!(a.last().map(String::as_str), Some("192.168.1.50"));
    }

    #[test]
    fn password_and_pairing_reach_the_command_line() {
        let opt = Options { password: Some("secret".into()), auth: Some("ABCD".into()), ..Options::seeded(7, 20) };
        let s = args(&homepod(), Path::new("/tmp/c"), &opt).join(" ");
        assert!(s.contains("--password secret --auth ABCD"), "{s}");
        assert!(s.contains("--dacp 0000000000000007"), "{s}");
        assert_eq!(pair_args(&homepod(), "0000000000000007").join(" "), "--pair-setup --port 7000 --dacp 0000000000000007 192.168.1.50");
        assert_eq!(parse_credentials("CREDENTIALS: 00ffAA\n"), Some("00ffAA"));
        assert_eq!(parse_credentials("CREDENTIALS: nope!"), None);
        assert_eq!(parse_credentials("Pairing failed."), None);
    }

    #[test]
    fn explains_pairing_errors() {
        assert!(explain("auth_required").contains("Allow Speaker & TV Access"));
        assert!(explain("whatever").contains("cliairplay"));
    }
}
