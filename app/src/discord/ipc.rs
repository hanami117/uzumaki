//! The local side of Discord's Rich Presence: the IPC socket the Discord desktop app listens on
//! (`discord-ipc-0` … `discord-ipc-9`), the frames it speaks and the handshake. Nothing here
//! touches the network: it is a Unix socket on this computer.
//!
//! A frame is `opcode: u32 LE`, `length: u32 LE`, then `length` bytes of JSON. Opcodes: 0
//! handshake, 1 frame, 2 close, 3 ping, 4 pong. We say `{"v":1,"client_id":"…"}`; Discord answers
//! with a frame whose `evt` is `READY` (or closes the socket: unknown client id, or it is not
//! ready yet).
//!
//! Linux and macOS: a Unix socket. Windows: Discord listens on the named pipe
//! `\\.\pipe\discord-ipc-N` (`pipe.rs` gives it the socket's read time limits); the frames and
//! the handshake are the same.

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};

pub const OP_HANDSHAKE: u32 = 0;
pub const OP_FRAME: u32 = 1;
pub const OP_CLOSE: u32 = 2;
pub const OP_PING: u32 = 3;
pub const OP_PONG: u32 = 4;

/// Discord's own sockets number 0 to 9 (a second Discord, say Canary, takes the next free one).
const SOCKETS: u32 = 10;
/// A frame larger than this is not Discord talking.
const MAX_FRAME: usize = 1 << 20;

/// One frame, ready to write.
pub fn encode(op: u32, payload: &Value) -> Vec<u8> {
    let body = payload.to_string().into_bytes();
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(&op.to_le_bytes());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// Reads one frame (opcode and its JSON; a body that isn't JSON reads as `null`).
pub fn read_frame(r: &mut impl Read) -> io::Result<(u32, Value)> {
    let mut head = [0u8; 8];
    r.read_exact(&mut head)?;
    finish_frame(r, head)
}

fn finish_frame(r: &mut impl Read, head: [u8; 8]) -> io::Result<(u32, Value)> {
    let op = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
    let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    Ok((op, serde_json::from_slice(&body).unwrap_or(Value::Null)))
}

/// The folders Discord may put its sockets in, from the environment, first choice first:
/// `$XDG_RUNTIME_DIR` (Linux), then `$TMPDIR` (macOS puts it there), `$TMP`, `$TEMP`, `/tmp`.
/// Under the runtime dir the Flatpak, Snap and Vesktop sandboxes keep theirs one level down.
/// `env` is `std::env::var_os` (a parameter so tests can give their own).
pub fn dirs(env: &dyn Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let var = |name: &str| env(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    let mut out: Vec<PathBuf> = Vec::new();
    let mut add = |p: PathBuf| {
        if !out.contains(&p) {
            out.push(p);
        }
    };
    if let Some(run) = var("XDG_RUNTIME_DIR") {
        add(run.clone());
        for sub in ["app/com.discordapp.Discord", "app/com.discordapp.DiscordCanary", "app/dev.vencord.Vesktop", "snap.discord", "snap.discord-canary"] {
            add(run.join(sub));
        }
    }
    for name in ["TMPDIR", "TMP", "TEMP"] {
        if let Some(p) = var(name) {
            add(p);
        }
    }
    add(PathBuf::from("/tmp"));
    out
}

/// Every socket path to try, in order: socket 0 in every folder, then socket 1, … On Windows:
/// the ten pipes.
pub fn candidates(env: &dyn Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    if cfg!(windows) {
        return pipe_names().into_iter().map(PathBuf::from).collect();
    }
    let dirs = dirs(env);
    (0..SOCKETS).flat_map(|n| dirs.iter().map(move |d| d.join(format!("discord-ipc-{n}")))).collect()
}

/// The named pipes Discord listens on, on Windows.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn pipe_names() -> Vec<String> {
    (0..SOCKETS).map(|n| format!(r"\\.\pipe\discord-ipc-{n}")).collect()
}

/// The connection under a [`Conn`]: a Unix socket, on Windows a named pipe.
#[cfg(unix)]
type Stream = std::os::unix::net::UnixStream;
#[cfg(windows)]
type Stream = super::pipe::PipeStream;

/// An open, handshaken connection.
pub struct Conn {
    stream: Stream,
    nonce: u64,
}

/// Why a connection failed, for the log (one line per outage).
pub fn describe(e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => "Discord isn't running".into(),
        _ => e.to_string(),
    }
}

impl Conn {
    /// Tries each path in turn; the first Discord that answers the handshake wins. The error is
    /// the last one seen (NotFound when no socket exists at all).
    pub fn connect(paths: &[PathBuf], client_id: &str, timeout: Duration) -> io::Result<Self> {
        let mut last = io::Error::new(io::ErrorKind::NotFound, "no Discord socket");
        for path in paths {
            match Self::open(path, client_id, timeout) {
                Ok(c) => return Ok(c),
                // A path that doesn't exist isn't news; a socket that refuses us or says
                // something odd is a better thing to report than "not found".
                Err(e) if e.kind() == io::ErrorKind::NotFound && last.kind() == io::ErrorKind::NotFound => last = e,
                Err(e) if e.kind() != io::ErrorKind::NotFound => last = e,
                Err(_) => {}
            }
        }
        Err(last)
    }

    fn open(path: &std::path::Path, client_id: &str, timeout: Duration) -> io::Result<Self> {
        let stream = Stream::connect(path)?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        let mut c = Conn { stream, nonce: 0 };
        c.stream.write_all(&encode(OP_HANDSHAKE, &json!({"v": 1, "client_id": client_id})))?;
        loop {
            match read_frame(&mut c.stream)? {
                (OP_FRAME, v) if v["evt"] == "READY" => return Ok(c),
                (OP_FRAME, v) if v["evt"] == "ERROR" => {
                    let msg = v["data"]["message"].as_str().unwrap_or("refused");
                    return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("Discord refused the handshake: {msg}")));
                }
                (OP_CLOSE, v) => {
                    let msg = v["message"].as_str().unwrap_or("closed");
                    return Err(io::Error::new(io::ErrorKind::ConnectionRefused, format!("Discord closed the handshake: {msg}")));
                }
                (OP_PING, v) => c.stream.write_all(&encode(OP_PONG, &v))?,
                _ => {}
            }
        }
    }

    /// Shows `activity`, or clears the presence when it is `None`. Waits for Discord's answer
    /// (bounded by the socket timeout). An error answer from Discord (a payload it dislikes) is
    /// `Ok(Some(message))`: the connection is fine, the caller logs it. An `Err` means the
    /// connection is gone.
    pub fn set_activity(&mut self, activity: Option<&Value>) -> io::Result<Option<String>> {
        self.nonce += 1;
        let nonce = self.nonce.to_string();
        let mut args = json!({"pid": std::process::id()});
        if let Some(a) = activity {
            args["activity"] = a.clone();
        }
        let frame = encode(OP_FRAME, &json!({"cmd": "SET_ACTIVITY", "args": args, "nonce": nonce}));
        self.stream.write_all(&frame)?;
        self.stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        loop {
            match read_frame(&mut self.stream)? {
                (OP_FRAME, v) if v["nonce"] == nonce.as_str() => {
                    return Ok((v["evt"] == "ERROR").then(|| v["data"]["message"].as_str().unwrap_or("an error").to_string()));
                }
                (OP_CLOSE, _) => return Err(io::Error::new(io::ErrorKind::ConnectionAborted, "Discord closed the connection")),
                (OP_PING, v) => self.stream.write_all(&encode(OP_PONG, &v))?,
                _ => {}
            }
        }
    }

    /// Answers whatever Discord sent unasked (pings) and notices it going away. Never waits
    /// more than a moment.
    pub fn poll(&mut self) -> io::Result<()> {
        self.stream.set_read_timeout(Some(Duration::from_millis(5)))?;
        loop {
            let mut head = [0u8; 8];
            let n = match self.stream.read(&mut head) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Discord went away")),
                Ok(n) => n,
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => return Ok(()),
                Err(e) => return Err(e),
            };
            // Part of a frame arrived: the rest follows at once.
            self.stream.set_read_timeout(Some(Duration::from_secs(2)))?;
            self.stream.read_exact(&mut head[n..])?;
            match finish_frame(&mut self.stream, head)? {
                (OP_PING, v) => self.stream.write_all(&encode(OP_PONG, &v))?,
                (OP_CLOSE, _) => return Err(io::Error::new(io::ErrorKind::ConnectionAborted, "Discord closed the connection")),
                _ => {}
            }
            self.stream.set_read_timeout(Some(Duration::from_millis(5)))?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| OsString::from(v))
    }

    #[test]
    fn a_frame_is_opcode_length_and_json() {
        let bytes = encode(OP_HANDSHAKE, &json!({"v": 1}));
        assert_eq!(&bytes[..4], &[0, 0, 0, 0]);
        assert_eq!(&bytes[4..8], &[7, 0, 0, 0]);
        assert_eq!(&bytes[8..], br#"{"v":1}"#);
        let (op, v) = read_frame(&mut &bytes[..]).unwrap();
        assert_eq!((op, v), (OP_HANDSHAKE, json!({"v": 1})));
    }

    #[test]
    fn a_huge_or_cut_frame_is_an_error_not_an_allocation() {
        let mut huge = Vec::new();
        huge.extend_from_slice(&1u32.to_le_bytes());
        huge.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(read_frame(&mut &huge[..]).unwrap_err().kind(), io::ErrorKind::InvalidData);
        let cut = &encode(OP_FRAME, &json!({"a": 1}))[..10];
        assert_eq!(read_frame(&mut &cut[..]).unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    #[cfg(unix)]
    fn linux_looks_in_the_runtime_dir_and_the_sandboxes_below_it() {
        let d = dirs(&env(&[("XDG_RUNTIME_DIR", "/run/user/1000"), ("TMPDIR", "/tmp")]));
        let s: Vec<_> = d.iter().map(|p| p.to_str().unwrap()).collect();
        assert_eq!(s[0], "/run/user/1000");
        assert!(s.contains(&"/run/user/1000/app/com.discordapp.Discord"), "Flatpak");
        assert!(s.contains(&"/run/user/1000/snap.discord"), "Snap");
        assert!(s.contains(&"/run/user/1000/app/dev.vencord.Vesktop"));
        assert_eq!(s.iter().filter(|p| **p == "/tmp").count(), 1, "TMPDIR and the /tmp fallback are one folder");
    }

    #[test]
    #[cfg(unix)]
    fn macos_looks_in_tmpdir() {
        let d = dirs(&env(&[("TMPDIR", "/var/folders/ab/cd/T/")]));
        assert_eq!(d[0], PathBuf::from("/var/folders/ab/cd/T/"));
        assert_eq!(candidates(&env(&[("TMPDIR", "/var/folders/ab/cd/T/")]))[0], PathBuf::from("/var/folders/ab/cd/T/discord-ipc-0"));
        assert!(dirs(&env(&[("XDG_RUNTIME_DIR", "")])).iter().all(|p| p.as_os_str() != ""), "an empty variable is no folder");
    }

    /// Windows: no folders, the ten pipes in order.
    #[test]
    #[cfg(windows)]
    fn windows_tries_the_ten_pipes() {
        let c = candidates(&env(&[("TEMP", r"C:\Temp")]));
        assert_eq!(c.len(), 10);
        assert_eq!(c[0], PathBuf::from(r"\\.\pipe\discord-ipc-0"));
        assert_eq!(c[9], PathBuf::from(r"\\.\pipe\discord-ipc-9"));
    }

    #[test]
    #[cfg(unix)]
    fn sockets_are_tried_zero_first_in_every_folder() {
        let c = candidates(&env(&[("XDG_RUNTIME_DIR", "/run/user/1000")]));
        let n = dirs(&env(&[("XDG_RUNTIME_DIR", "/run/user/1000")])).len();
        assert_eq!(c.len(), n * 10);
        assert!(c[..n].iter().all(|p| p.ends_with("discord-ipc-0")));
        assert!(c[n..2 * n].iter().all(|p| p.ends_with("discord-ipc-1")));
        assert_eq!(pipe_names()[3], r"\\.\pipe\discord-ipc-3");
    }
}
