//! The Chromecast spike's tool: finds Cast devices and plays a test signal to one, logging what
//! the measurements need (fetch, start delay, the device's buffer at every status).
//!
//! ```text
//! uzumaki-cast list [--seconds N]
//! uzumaki-cast play <name> [--host IP[:PORT]] [--beeps | --tone | --file F.wav] [--flac]
//!                          [--buffered] [--volume N] [--port P] [--ipv4 | --ipv6]
//!                          [--preroll MS | --burst BYTES] [--http close|chunked|length]
//!                          [--rate 44.1k|48k|88.2k|96k] [--bits 16|24]
//! Defaults: WAV 96k/24, HTTP length, pre-roll = 1.4 MB of the stream (starts at once).
//! ```
//! While playing, type a letter and Enter: `p` pause, `r` resume, `l` reload (a new LOAD, as a
//! seek would), `v N` device volume 0-100, `s` status, `q` stop.
//! `--beeps` (default): a short beep each second; the tool prints `beep` as it sends one, so
//! the delay until it is heard is the latency, by ear. Nothing is played without `play`.

use std::io::BufRead;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use uzumaki_cast::{CastDevice, Config, Controller, Event, Format, Meta, State, pcm::Hub};

enum Signal {
    Beeps,
    Tone,
    File(Vec<i16>),
}

fn usage() -> ! {
    eprintln!("usage: uzumaki-cast list [--seconds N]\n       uzumaki-cast play <name> [--host IP[:PORT]] [--beeps|--tone|--file F.wav] [--flac] [--buffered] [--volume N] [--port P] [--ipv4|--ipv6] [--preroll MS|--burst BYTES] [--http close|chunked|length] [--rate R] [--bits B]");
    std::process::exit(2)
}

/// A 16-bit PCM WAV, 44.1 kHz, mono or stereo (convert others with `afconvert`/`ffmpeg`).
fn read_wav(path: &str) -> Result<Vec<i16>, String> {
    let b = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    if b.len() < 12 || &b[..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Err(format!("{path}: not a WAV file"));
    }
    let (mut at, mut fmt, mut data) = (12, None, None);
    while at + 8 <= b.len() {
        let id = &b[at..at + 4];
        let len = u32::from_le_bytes([b[at + 4], b[at + 5], b[at + 6], b[at + 7]]) as usize;
        let body = &b[at + 8..(at + 8 + len).min(b.len())];
        if id == b"fmt " && body.len() >= 16 {
            fmt = Some((u16::from_le_bytes([body[0], body[1]]), u16::from_le_bytes([body[2], body[3]]), u32::from_le_bytes([body[4], body[5], body[6], body[7]]), u16::from_le_bytes([body[14], body[15]])));
        } else if id == b"data" {
            data = Some(body);
        }
        at += 8 + len + (len & 1);
    }
    let (Some((1, ch @ (1 | 2), 44100, 16)), Some(data)) = (fmt, data) else {
        return Err(format!("{path}: needs 16-bit PCM at 44.1 kHz, mono or stereo (try: afconvert -f WAVE -d LEI16@44100 in.m4a out.wav)"));
    };
    let s: Vec<i16> = data.as_chunks::<2>().0.iter().map(|x| i16::from_le_bytes(*x)).collect();
    Ok(if ch == 1 { s.iter().flat_map(|&v| [v, v]).collect() } else { s })
}

/// Publishes the signal in real time (a little ahead of the clock), as the bridge's capture would.
/// While the controller holds (paused on the TV), the signal waits too: its clock moves on by
/// the pause, so after `r` it continues where the TV stopped, without a burst.
fn feed(hub: Arc<Hub>, signal: Signal, alive: Arc<AtomicBool>, ctl: Controller) {
    const CHUNK: usize = 4096;
    let mut start = Instant::now();
    let mut frame = 0u64;
    while alive.load(Ordering::SeqCst) {
        if ctl.holding() {
            std::thread::sleep(Duration::from_millis(20));
            start += Duration::from_millis(20);
            continue;
        }
        let due = start + Duration::from_secs_f64(frame as f64 / 44100.0);
        let now = Instant::now();
        if due > now + Duration::from_millis(100) {
            std::thread::sleep(due - now - Duration::from_millis(100));
            continue;
        }
        let mut chunk = Vec::with_capacity(CHUNK * 2);
        for i in 0..CHUNK as u64 {
            let f = frame + i;
            let t = f as f64 / 44100.0;
            let v = match &signal {
                Signal::Tone => ((t * 440.0 * std::f64::consts::TAU).sin() * 3000.0) as i16,
                Signal::Beeps => {
                    if f % 44100 < 4410 {
                        if f.is_multiple_of(44100) {
                            println!("beep (stream time {:.0} s)", t);
                        }
                        ((t * 1000.0 * std::f64::consts::TAU).sin() * 3000.0) as i16
                    } else {
                        0
                    }
                }
                Signal::File(s) => {
                    let n = (s.len() / 2) as u64;
                    let at = (f % n.max(1)) as usize * 2;
                    chunk.push(s[at]);
                    chunk.push(s[at + 1]);
                    continue;
                }
            };
            chunk.push(v);
            chunk.push(v);
        }
        let bytes: Vec<u8> = chunk.iter().flat_map(|s| s.to_le_bytes()).collect();
        hub.publish(&bytes);
        frame += CHUNK as u64;
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().any(|a| a == name);
    let value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let mut cfg = Config::from_env();
    cfg.app_inputs = false;
    match args.first().map(String::as_str) {
        Some("list") => {
            let secs = value("--seconds").and_then(|s| s.parse().ok()).unwrap_or(4);
            match uzumaki_cast::discover::discover(Duration::from_secs(secs), None) {
                Ok(list) if list.is_empty() => println!("no Cast devices found in {secs} s (mDNS _googlecast._tcp)"),
                Ok(list) => {
                    for d in list {
                        let more = if d.addrs.len() > 1 { format!(" (also {})", d.addrs.iter().filter(|a| **a != d.ip).map(ToString::to_string).collect::<Vec<_>>().join(", ")) } else { String::new() };
                        println!("{:<28} {:<24} {}:{}{more}{}{}", d.name, d.model, d.ip, d.port, if d.group { "  [group]" } else { "" }, if d.playing.is_empty() { String::new() } else { format!("  (playing: {})", d.playing) });
                    }
                }
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            }
        }
        Some("play") => {
            let Some(name) = args.get(1).filter(|a| !a.starts_with("--")).cloned() else { usage() };
            if flag("--flac") {
                cfg.format = Format::Flac;
            }
            if flag("--ipv4") {
                cfg.family = uzumaki_cast::Family::V4;
            } else if flag("--ipv6") {
                cfg.family = uzumaki_cast::Family::V6;
            }
            if let Some(ms) = value("--preroll").and_then(|v| v.parse::<u64>().ok()) {
                cfg.preroll_ms = Some(ms.min(10_000));
            }
            if let Some(b) = value("--burst").and_then(|v| v.parse::<u64>().ok()) {
                cfg.burst_bytes = b;
            }
            if value("--rate").is_some() || value("--bits").is_some() {
                let rate = value("--rate").and_then(|r| uzumaki_cast::wav::Spec::parse_rate(&r)).unwrap_or(cfg.wav.rate);
                let bits = value("--bits").and_then(|b| b.parse().ok()).unwrap_or(cfg.wav.bits);
                match uzumaki_cast::wav::Spec::new(rate, bits) {
                    Some(s) => cfg.wav = s,
                    None => {
                        eprintln!("--rate takes 44.1k, 48k, 88.2k or 96k; --bits 16 or 24");
                        std::process::exit(2);
                    }
                }
            }
            if let Some(h) = value("--http") {
                match uzumaki_cast::Http::parse(&h) {
                    Some(h) => cfg.http = h,
                    None => usage(),
                }
            }
            if flag("--buffered") {
                cfg.stream_type = "BUFFERED".into();
            }
            if let Some(p) = value("--port").and_then(|p| p.parse().ok()) {
                cfg.port = p;
            }
            if let Some(h) = value("--host") {
                let (ip, port) = match h.rsplit_once(':').filter(|(a, _)| !a.contains(':') || a.starts_with('[')) {
                    Some((a, p)) => (a.trim_matches(['[', ']']).to_string(), p.parse().unwrap_or(8009)),
                    None => (h.clone(), 8009),
                };
                let Ok(ip) = ip.parse::<IpAddr>() else { usage() };
                cfg.fake = Some(CastDevice::at(ip, port, &name));
            }
            let signal = if let Some(f) = value("--file") {
                match read_wav(&f) {
                    Ok(s) if !s.is_empty() => Signal::File(s),
                    Ok(_) => usage(),
                    Err(e) => {
                        eprintln!("{e}");
                        std::process::exit(1);
                    }
                }
            } else if flag("--tone") {
                Signal::Tone
            } else {
                Signal::Beeps
            };
            let volume = value("--volume").and_then(|v| v.parse::<u8>().ok());
            let done = Arc::new(AtomicBool::new(false));
            let d = done.clone();
            let c = Controller::new(cfg, move |e| match e {
                Event::State(State::Off) => d.store(true, Ordering::SeqCst),
                Event::State(s) => println!("state: {}", s.label()),
                Event::Failed { name, why, detail } => println!("failed: {} ({detail})", why.message(&name)),
                Event::Volume { level, muted } => println!("device volume {:.0}%{}", level * 100.0, if muted { " (muted)" } else { "" }),
                _ => {}
            });
            let alive = Arc::new(AtomicBool::new(true));
            {
                let (hub, alive) = (c.hub(), alive.clone());
                let ctl = c.clone();
                std::thread::spawn(move || feed(hub, signal, alive, ctl));
            }
            c.connect_named(&name, Meta { title: "Uzumaki Cast test".into(), artist: "Uzumaki".into(), ..Meta::default() });
            if let Some(v) = volume {
                // After the session is up (the command goes to it).
                let c2 = c.clone();
                std::thread::spawn(move || {
                    while !c2.set_volume(v) {
                        std::thread::sleep(Duration::from_millis(200));
                    }
                });
            }
            let (ltx, lrx) = std::sync::mpsc::channel::<String>();
            std::thread::spawn(move || {
                for line in std::io::stdin().lock().lines().map_while(Result::ok) {
                    if ltx.send(line).is_err() {
                        break;
                    }
                }
            });
            println!("keys: p pause, r resume, l reload, v N volume, s status, q stop (then Enter)");
            loop {
                if done.load(Ordering::SeqCst) && c.idle() {
                    break;
                }
                let Ok(line) = lrx.recv_timeout(Duration::from_millis(200)) else { continue };
                let mut w = line.split_whitespace();
                match w.next() {
                    Some("p") => _ = c.pause(),
                    Some("r") => _ = c.resume(),
                    Some("l") => _ = c.reload(None),
                    Some("v") => match w.next().and_then(|v| v.parse::<u8>().ok()) {
                        Some(v) => _ = c.set_volume(v),
                        None => println!("v takes 0-100"),
                    },
                    Some("s") => c.status_lines().iter().for_each(|l| println!("{l}")),
                    Some("q") => {
                        c.stop();
                        let t = Instant::now();
                        while !c.idle() && t.elapsed() < Duration::from_secs(3) {
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        break;
                    }
                    _ => {}
                }
            }
            c.status_lines().iter().for_each(|l| println!("{l}"));
            alive.store(false, Ordering::SeqCst);
            c.shutdown();
        }
        _ => usage(),
    }
}
