//! The Windows transport: a named pipe instead of a Unix socket.
//!
//! The pipe is `\\.\pipe\io.github.hanami117.uzumaki-<user SID>`. Only the current user may open
//! it, remote clients are refused, and the first instance is created with
//! `FILE_FLAG_FIRST_PIPE_INSTANCE`, so a second app can't take the name: that is single instance.
//! Handles are plain blocking ones; a connection reads a line, then writes a line, on one thread.
//! A blocking read has no time limit of its own, so the client's wait for a reply is watched by
//! a second thread that cancels it when the limit passes (`with_deadline`).

use std::fs::File;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, ERROR_OPERATION_ABORTED, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE, LocalFree};
use windows_sys::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows_sys::Win32::Security::{GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX, SECURITY_IDENTIFICATION};
use windows_sys::Win32::System::IO::CancelIoEx;
use windows_sys::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, WaitNamedPipeW};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use crate::{Reply, Request};

const PREFIX: &str = r"\\.\pipe\";

/// The pipe of the current user.
pub fn default_path() -> PathBuf {
    let who = user_sid().or_else(|| std::env::var("USERNAME").ok()).unwrap_or_default();
    PathBuf::from(format!("{PREFIX}io.github.hanami117.uzumaki-{who}"))
}

/// A file-name-safe text for a pipe path (`uzu`'s `.last` record is named after it).
pub fn file_stem(path: &Path) -> String {
    let name = pipe_name(path);
    name[PREFIX.len().min(name.len())..].chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '-' }).collect()
}

/// A pipe name for any path: a `\\.\pipe\` path as it is, anything else (a `UZUMAKI_CTL_SOCKET`
/// written like a file path) as a name made from its text.
pub fn pipe_name(path: &Path) -> String {
    let text = path.to_string_lossy();
    if text.to_ascii_lowercase().starts_with(PREFIX) {
        return text.into_owned();
    }
    let name: String = text.chars().map(|c| if matches!(c, '\\' | '/' | ':') { '-' } else { c }).collect();
    format!("{PREFIX}{name}")
}

fn wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s).encode_wide().chain(Some(0)).collect()
}

/// The current user's SID as text (`S-1-5-21-…`).
fn user_sid() -> Option<String> {
    // SAFETY: plain Win32 calls with buffers that outlive them; the token handle is closed and
    // the string the system allocates is freed with LocalFree, as its documentation asks.
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        // A TOKEN_USER and its SID (at most 68 bytes), 8-byte aligned.
        let mut buf = [0u64; 32];
        let mut len = 0u32;
        let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), std::mem::size_of_val(&buf) as u32, &mut len);
        CloseHandle(token);
        if ok == 0 {
            return None;
        }
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut text: *mut u16 = std::ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 || text.is_null() {
            return None;
        }
        let mut n = 0;
        while *text.add(n) != 0 {
            n += 1;
        }
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, n));
        LocalFree(text.cast());
        Some(sid)
    }
}

/// A security descriptor that lets only the current user in. Kept for the life of the process
/// (one small allocation per `serve`).
struct OnlyMe(PSECURITY_DESCRIPTOR);
// SAFETY: the descriptor is read-only memory the system allocated; nothing mutates it.
unsafe impl Send for OnlyMe {}

impl OnlyMe {
    fn new() -> OnlyMe {
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        if let Some(sid) = user_sid() {
            let sddl = wide(&format!("D:P(A;;GA;;;{sid})"));
            // SAFETY: `sddl` is NUL-terminated; `sd` receives a LocalAlloc'd descriptor or stays null.
            if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, std::ptr::null_mut()) } == 0 {
                sd = std::ptr::null_mut();
            }
        }
        // Null means the default descriptor: the owner has full control, others can't write.
        OnlyMe(sd)
    }
}

fn instance(name: &[u16], first: bool, who: &OnlyMe) -> io::Result<File> {
    let attrs = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: who.0, bInheritHandle: 0 };
    let open = PIPE_ACCESS_DUPLEX | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
    let mode = PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS;
    // SAFETY: `name` is NUL-terminated and `attrs` lives across the call.
    let h = unsafe { CreateNamedPipeW(name.as_ptr(), open, mode, PIPE_UNLIMITED_INSTANCES, 16 * 1024, 16 * 1024, 0, if who.0.is_null() { std::ptr::null() } else { &attrs }) };
    if h == INVALID_HANDLE_VALUE {
        let e = io::Error::last_os_error();
        // The name is taken: another app answers there.
        if first && e.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) {
            return Err(io::Error::new(io::ErrorKind::AddrInUse, "the pipe is in use"));
        }
        return Err(e);
    }
    // SAFETY: `h` is a valid handle we own.
    Ok(unsafe { File::from_raw_handle(h) })
}

/// Waits for a client on `pipe`.
fn accept(pipe: &File) -> bool {
    // SAFETY: a valid pipe handle, no overlapped I/O.
    let ok = unsafe { ConnectNamedPipe(pipe.as_raw_handle(), std::ptr::null_mut()) } != 0;
    // A client that connected between the create and this call is fine too.
    ok || io::Error::last_os_error().raw_os_error() == Some(ERROR_PIPE_CONNECTED as i32)
}

/// The listening side; [`Listener::stop`] ends its thread and frees the name.
pub struct Listener {
    name: String,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Listener {
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the accept thread: it sleeps in ConnectNamedPipe.
        let _ = std::fs::OpenOptions::new().read(true).write(true).open(&self.name);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub fn listen(path: &Path, handler: Arc<dyn Fn(Request) -> Reply + Send + Sync>) -> io::Result<Listener> {
    let name = pipe_name(path);
    let wname = wide(&name);
    let who = OnlyMe::new();
    let mut next = instance(&wname, true, &who)?;
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let thread = std::thread::Builder::new().name("uzumaki-ctl".into()).spawn(move || {
        loop {
            let connected = accept(&next);
            if stopped.load(Ordering::SeqCst) {
                return;
            }
            // The next instance exists before this one is served, so a client never finds no pipe.
            let conn = match instance(&wname, false, &who) {
                Ok(pipe) => std::mem::replace(&mut next, pipe),
                Err(e) => {
                    eprintln!("uzumaki: the control pipe stopped: {e}");
                    return;
                }
            };
            if connected {
                let handler = handler.clone();
                let _ = std::thread::Builder::new().name("uzumaki-ctl-conn".into()).spawn(move || crate::connection(conn, &*handler));
            }
        }
    })?;
    Ok(Listener { name, stop, thread: Some(thread) })
}

/// Opens the pipe as a client. `NotFound` when nobody listens (Uzumaki isn't running).
pub fn connect(path: &Path) -> io::Result<File> {
    let name = pipe_name(path);
    let wname = wide(&name);
    // About 3 seconds in all: the app always keeps a spare instance, but it makes the next one only
    // after it accepts a client, so a burst of clients can wait a moment. Longer means it is going away.
    let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        // SECURITY_IDENTIFICATION: the server may see who we are, and can't act as us.
        match std::fs::OpenOptions::new().read(true).write(true).security_qos_flags(SECURITY_IDENTIFICATION).open(&name) {
            Ok(f) => return Ok(f),
            // Every instance is busy for a moment: wait for a free one.
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) && std::time::Instant::now() < until => {
                // SAFETY: `wname` is NUL-terminated.
                unsafe { WaitNamedPipeW(wname.as_ptr(), 250) };
                // WaitNamedPipe can return at once when no instance is listening yet.
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(e) => return Err(e),
        }
    }
}

/// Runs `io` (blocking reads or writes on `pipe`, or on a copy of its handle) and gives up after
/// `limit`: a watcher thread then cancels whatever the pipe is waiting in, and the answer is
/// `TimedOut`. The watcher sleeps until then and ends with the call; nothing polls.
pub fn with_deadline<T>(pipe: &File, limit: std::time::Duration, io: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    // A raw handle isn't `Send`; the number is. `pipe` outlives the watcher (joined below).
    let handle = pipe.as_raw_handle() as usize;
    let fired = Arc::new(AtomicBool::new(false));
    let (done, wait) = std::sync::mpsc::channel::<()>();
    let watcher = {
        let fired = fired.clone();
        std::thread::Builder::new().name("uzumaki-ctl-deadline".into()).spawn(move || {
            if wait.recv_timeout(limit) == Err(std::sync::mpsc::RecvTimeoutError::Timeout) {
                fired.store(true, Ordering::SeqCst);
                // SAFETY: the handle is open until this thread is joined; a null OVERLAPPED
                // cancels every pending operation on it, from any thread.
                unsafe { CancelIoEx(handle as HANDLE, std::ptr::null()) };
            }
        })
    };
    let result = io();
    drop(done);
    if let Ok(w) = watcher {
        let _ = w.join();
    }
    match result {
        Err(e) if fired.load(Ordering::SeqCst) && e.raw_os_error() == Some(ERROR_OPERATION_ABORTED as i32) => {
            Err(io::Error::new(io::ErrorKind::TimedOut, format!("Uzumaki didn't answer within {} seconds", limit.as_secs().max(1))))
        }
        other => other,
    }
}
