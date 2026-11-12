//! uzumaki-cli: launches the Electron playback bridge and lets you type commands.
//!
//!   cargo run -- [--show] [--verbose]
//!
//! --show     start with the Apple Music window visible (same as typing `login`)
//! --verbose  also print time ticks, minor state changes and non-JSON bridge output
//!
//! Bridge location: $UZUMAKI_BRIDGE_DIR (or the old $CADENCE_BRIDGE_DIR), default ../bridge next to this crate.
//! Electron's stderr goes to a log file (path printed at startup).

use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const HELP: &str = "\
commands:
  search <words>      search the catalog for songs
  play <n|id> [...]   play search result #n or song id(s); several make a queue
  play                resume
  pause | next | prev
  seek <seconds>
  vol <0..1>
  status              what's playing, position, signed-in state
  login               show the Apple Music window (sign in there, then `hide`)
  hide                hide the window again
  widevine            check that the page can get a Widevine key system
  raw <json>          send a raw bridge command, e.g.
                      raw {\"cmd\":\"api\",\"path\":\"/v1/me/library/songs\",\"params\":{\"limit\":5}}
  quit";

#[derive(Default)]
struct Shared {
    pending: HashMap<u64, String>, // request id -> command name, to format replies
    results: Vec<String>,          // song ids from the last search
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let show = args.iter().any(|a| a == "--show");
    let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");

    let dir = std::env::var_os("UZUMAKI_BRIDGE_DIR").or_else(|| std::env::var_os("CADENCE_BRIDGE_DIR"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../bridge"));
    let dir = dir.canonicalize().unwrap_or(dir);
    // Inside the app bundle on macOS, a plain binary on Linux (the same rule as the app's bridge.rs).
    let dist = dir.join("node_modules/electron/dist");
    let electron = if cfg!(target_os = "macos") {
        dist.join("Electron.app/Contents/MacOS/Electron")
    } else {
        dist.join("electron")
    };
    if !electron.exists() {
        eprintln!("castLabs Electron not found at {}", electron.display());
        eprintln!(
            "run `npm install && node node_modules/electron/install.js` in {}",
            dir.display()
        );
        std::process::exit(1);
    }

    let log_path = std::env::temp_dir().join("uzumaki-bridge.log");
    let log = std::fs::File::create(&log_path).expect("cannot create bridge log file");

    let mut cmd = Command::new(&electron);
    cmd.arg(&dir);
    if show {
        cmd.arg("--show");
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(log)
        .spawn()
        .expect("failed to start Electron");

    println!("starting bridge (log: {})", log_path.display());
    println!("type `help` for commands");

    let shared = Arc::new(Mutex::new(Shared::default()));
    let stdout = child.stdout.take().unwrap();
    let reader_shared = shared.clone();
    thread::spawn(move || read_bridge(stdout, reader_shared, verbose));

    let mut to_bridge = child.stdin.take().unwrap();
    let mut next_id: u64 = 1;
    let mut send = |cmd: Value| -> io::Result<()> {
        let id = next_id;
        next_id += 1;
        let name = cmd["cmd"].as_str().unwrap_or("?").to_string();
        shared.lock().unwrap().pending.insert(id, name);
        let mut msg = cmd;
        msg["id"] = json!(id);
        writeln!(to_bridge, "{msg}")?;
        to_bridge.flush()
    };

    for line in io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (word, rest) = match line.split_once(char::is_whitespace) {
            Some((w, r)) => (w, r.trim()),
            None => (line, ""),
        };
        let cmd = match word {
            "help" | "?" => {
                println!("{HELP}");
                continue;
            }
            "search" | "s" if !rest.is_empty() => {
                json!({"cmd": "search", "term": rest, "limit": 10})
            }
            "play" | "p" if rest.is_empty() => json!({"cmd": "play"}),
            "play" | "p" => {
                let results = shared.lock().unwrap().results.clone();
                let ids: Vec<String> = rest
                    .split_whitespace()
                    .map(|a| resolve(a, &results))
                    .collect();
                json!({"cmd": "setQueue", "songIds": ids})
            }
            "pause" => json!({"cmd": "pause"}),
            "next" | "n" => json!({"cmd": "next"}),
            "prev" | "previous" => json!({"cmd": "previous"}),
            "seek" => match rest.parse::<f64>() {
                Ok(t) => json!({"cmd": "seek", "time": t}),
                Err(_) => {
                    println!("usage: seek <seconds>");
                    continue;
                }
            },
            "vol" | "volume" => match rest.parse::<f64>() {
                Ok(v) => json!({"cmd": "volume", "value": v}),
                Err(_) => {
                    println!("usage: vol <0..1>");
                    continue;
                }
            },
            "status" | "st" => json!({"cmd": "status"}),
            "login" | "show" => json!({"cmd": "show"}),
            "hide" => json!({"cmd": "hide"}),
            "widevine" => json!({"cmd": "widevine"}),
            "raw" => match serde_json::from_str::<Value>(rest) {
                Ok(v) if v.is_object() => v,
                _ => {
                    println!("usage: raw {{\"cmd\": ...}}");
                    continue;
                }
            },
            "quit" | "exit" | "q" => break,
            _ => {
                println!("unknown command `{word}` (type `help`)");
                continue;
            }
        };
        if send(cmd).is_err() {
            println!("bridge is gone");
            break;
        }
    }

    // Ask the bridge to quit, then make sure it's gone.
    let _ = send(json!({"cmd": "quit"}));
    drop(send);
    drop(to_bridge); // EOF on the bridge's stdin also makes it quit
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// `3` -> id of search result #3; anything else is taken as a song id.
fn resolve(arg: &str, results: &[String]) -> String {
    match arg.parse::<usize>() {
        Ok(n) if n >= 1 && n <= results.len() => results[n - 1].clone(),
        _ => arg.to_string(),
    }
}

fn read_bridge(stdout: impl io::Read, shared: Arc<Mutex<Shared>>, verbose: bool) {
    let mut last_state = String::new();
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            if verbose {
                println!("[bridge] {line}");
            }
            continue;
        };
        if let Some(event) = msg["event"].as_str() {
            on_event(event, &msg, verbose, &mut last_state);
        } else if let Some(id) = msg["id"].as_u64() {
            let cmd = shared
                .lock()
                .unwrap()
                .pending
                .remove(&id)
                .unwrap_or_default();
            on_reply(&cmd, &msg, &shared);
        }
    }
    println!("bridge exited");
    std::process::exit(0);
}

fn on_event(event: &str, msg: &Value, verbose: bool, last_state: &mut String) {
    match event {
        "widevine" if msg["ok"].as_bool() == Some(true) => {
            let version = msg["status"]
                .as_object()
                .and_then(|m| m.values().find_map(|c| c["version"].as_str()))
                .unwrap_or("?");
            println!("Widevine CDM ready ({version})");
        }
        "widevine" => println!("Widevine CDM FAILED: {}", msg["error"]),
        "ready" if msg["authorized"].as_bool() == Some(true) => {
            println!(
                "bridge ready, signed in (storefront {}). Try: search <words>",
                str_or(&msg["storefront"], "?")
            );
        }
        "ready" => {
            println!(
                "bridge ready, NOT signed in. Type `login`, sign in in the window, then `hide`."
            );
            println!("(signed out, Apple only plays 90-second previews)");
        }
        "auth" if msg["authorized"].as_bool() == Some(true) => println!("signed in"),
        "auth" => println!("signed out"),
        "nowPlaying" if !msg["item"].is_null() => println!("♪ {}", describe(&msg["item"])),
        "nowPlaying" => {}
        "playbackState" => {
            // MusicKit goes playing -> waiting -> loading -> playing on start; show each change once.
            let state = str_or(&msg["state"], "?");
            if verbose
                || (matches!(
                    state,
                    "playing" | "paused" | "stopped" | "ended" | "completed"
                ) && state != last_state)
            {
                println!("[{state}]");
                *last_state = state.to_string();
            }
        }
        "time" if verbose => {
            println!(
                "  {} / {}",
                mmss(msg["current"].as_f64().unwrap_or(0.0)),
                mmss(msg["duration"].as_f64().unwrap_or(0.0))
            );
        }
        "error" => println!("error: {}", str_or(&msg["message"], "?")),
        _ if verbose => println!("event: {msg}"),
        _ => {}
    }
}

fn on_reply(cmd: &str, msg: &Value, shared: &Mutex<Shared>) {
    if msg["ok"].as_bool() != Some(true) {
        println!("error ({cmd}): {}", str_or(&msg["error"], "unknown error"));
        return;
    }
    let result = &msg["result"];
    match cmd {
        "search" => {
            let songs = result["songs"].as_array().cloned().unwrap_or_default();
            if songs.is_empty() {
                println!("no results");
            }
            for (i, s) in songs.iter().enumerate() {
                println!(
                    "{:>2}. {}  [{}]  {}",
                    i + 1,
                    describe(s),
                    mmss(s["duration"].as_f64().unwrap_or(0.0)),
                    str_or(&s["id"], "?")
                );
            }
            shared.lock().unwrap().results = songs
                .iter()
                .filter_map(|s| s["id"].as_str().map(String::from))
                .collect();
            if !songs.is_empty() {
                println!("play <n> to play one");
            }
        }
        "status" => {
            println!(
                "signed in: {}  storefront: {}  state: {}  queue: {}  volume: {}",
                result["authorized"].as_bool().unwrap_or(false),
                str_or(&result["storefront"], "?"),
                str_or(&result["state"], "?"),
                result["queueLength"],
                result["volume"]
            );
            if !result["nowPlaying"].is_null() {
                println!(
                    "♪ {}  {} / {}",
                    describe(&result["nowPlaying"]),
                    mmss(result["current"].as_f64().unwrap_or(0.0)),
                    mmss(result["duration"].as_f64().unwrap_or(0.0))
                );
            }
        }
        "widevine" => println!("widevine: {result}"),
        _ if !result.is_null() && cmd != "setQueue" => {
            println!(
                "{}",
                serde_json::to_string_pretty(result).unwrap_or_default()
            );
        }
        _ => {}
    }
}

fn describe(item: &Value) -> String {
    format!(
        "{} — {} ({})",
        str_or(&item["title"], "?"),
        str_or(&item["artist"], "?"),
        str_or(&item["album"], "?")
    )
}

fn str_or<'a>(v: &'a Value, default: &'a str) -> &'a str {
    v.as_str().unwrap_or(default)
}

fn mmss(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}
