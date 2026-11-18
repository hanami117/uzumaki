//! Windows: Discord's IPC is a named pipe (`\\.\pipe\discord-ipc-N`) instead of a Unix socket.
//! `PipeStream` gives it the few things `ipc::Conn` asks of a socket: reads that give up after a
//! time limit, and writes.
//!
//! A pipe handle opened the plain way has no read timeout, so a read first asks how many bytes
//! wait (`PeekNamedPipe`, which never blocks) and sleeps a few milliseconds between looks, until
//! the limit passes. Discord's messages are tiny and rare; nothing here spins.

use std::cell::Cell;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::time::{Duration, Instant};

use windows_sys::Win32::System::Pipes::PeekNamedPipe;

pub struct PipeStream {
    file: File,
    read_timeout: Cell<Option<Duration>>,
}

impl PipeStream {
    /// Opens Discord's pipe. `NotFound` when Discord isn't running.
    pub fn connect(path: &Path) -> io::Result<Self> {
        let file = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
        Ok(Self { file, read_timeout: Cell::new(None) })
    }

    pub fn set_read_timeout(&self, limit: Option<Duration>) -> io::Result<()> {
        self.read_timeout.set(limit);
        Ok(())
    }

    /// Writes to Discord's pipe don't wait (its buffer is far larger than a presence).
    pub fn set_write_timeout(&self, _limit: Option<Duration>) -> io::Result<()> {
        Ok(())
    }

    /// How many bytes can be read without waiting. An error when the other end is gone.
    fn waiting(&self) -> io::Result<u32> {
        let mut available = 0u32;
        // SAFETY: a valid pipe handle; only the "bytes available" out pointer is asked for.
        let ok = unsafe { PeekNamedPipe(self.file.as_raw_handle() as _, std::ptr::null_mut(), 0, std::ptr::null_mut(), &mut available, std::ptr::null_mut()) };
        if ok == 0 { Err(io::Error::last_os_error()) } else { Ok(available) }
    }
}

impl Read for PipeStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let Some(limit) = self.read_timeout.get() else { return self.file.read(buf) };
        let start = Instant::now();
        loop {
            match self.waiting() {
                Ok(0) => {}
                Ok(n) => {
                    let n = (n as usize).min(buf.len());
                    return self.file.read(&mut buf[..n]);
                }
                // The other end closed: the same as a socket's end of stream.
                Err(e) if e.kind() == io::ErrorKind::BrokenPipe => return Ok(0),
                Err(e) => return Err(e),
            }
            let left = limit.saturating_sub(start.elapsed());
            if left.is_zero() {
                return Err(io::ErrorKind::TimedOut.into());
            }
            std::thread::sleep(left.min(Duration::from_millis(10)));
        }
    }
}

impl Write for PipeStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discord::ipc::{self, Conn};
    use serde_json::json;
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
    use windows_sys::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT};

    /// A stand-in for Discord: one pipe instance under a name of this test.
    fn server(name: &str) -> File {
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        // SAFETY: a NUL-terminated name; the handle is owned by the File.
        unsafe {
            let h = CreateNamedPipeW(wide.as_ptr(), PIPE_ACCESS_DUPLEX, PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT, 1, 4096, 4096, 0, std::ptr::null());
            assert!(h != INVALID_HANDLE_VALUE, "{}", io::Error::last_os_error());
            File::from_raw_handle(h as _)
        }
    }

    #[test]
    fn the_handshake_a_presence_and_discord_leaving_over_a_real_pipe() {
        let name = format!(r"\\.\pipe\uzumaki-discord-test-{}", std::process::id());
        let pipe = server(&name);
        let discord = std::thread::spawn(move || {
            let mut pipe = pipe;
            // SAFETY: a valid pipe handle; waits for the client.
            unsafe { ConnectNamedPipe(pipe.as_raw_handle() as _, std::ptr::null_mut()) };
            let (op, hello) = ipc::read_frame(&mut pipe).unwrap();
            assert_eq!((op, hello["client_id"].as_str()), (ipc::OP_HANDSHAKE, Some("123")));
            pipe.write_all(&ipc::encode(ipc::OP_FRAME, &json!({"cmd": "DISPATCH", "evt": "READY"}))).unwrap();
            let (op, set) = ipc::read_frame(&mut pipe).unwrap();
            assert_eq!((op, set["cmd"].as_str(), set["args"]["activity"]["details"].as_str()), (ipc::OP_FRAME, Some("SET_ACTIVITY"), Some("Song")));
            pipe.write_all(&ipc::encode(ipc::OP_FRAME, &json!({"cmd": "SET_ACTIVITY", "nonce": set["nonce"], "evt": null}))).unwrap();
            // Discord quits: the pipe closes.
        });
        let mut conn = Conn::connect(&[std::path::PathBuf::from(r"\\.\pipe\uzumaki-discord-test-nobody"), std::path::PathBuf::from(&name)], "123", Duration::from_secs(5)).unwrap();
        assert_eq!(conn.set_activity(Some(&json!({"details": "Song"}))).unwrap(), None);
        discord.join().unwrap();
        // Nothing waits and then the other end is gone: a quiet poll first or not, it ends in an error.
        let gone = (0..200).find_map(|_| {
            std::thread::sleep(Duration::from_millis(10));
            conn.poll().err()
        });
        assert!(gone.is_some(), "a closed pipe is noticed");
    }

    #[test]
    fn no_discord_is_not_found() {
        let e = Conn::connect(&[std::path::PathBuf::from(r"\\.\pipe\uzumaki-discord-test-nobody")], "123", Duration::from_secs(1)).err().expect("nobody listens");
        assert_eq!(ipc::describe(&e), "Discord isn't running");
    }
}
