//! Where there are no Unix sockets (Windows): AirPlay is not available. `helper::find` finds no
//! helper there, so no session ever starts; these stand-ins only let the crate compile, and every
//! call fails.

use std::io::{self, Read};
use std::path::Path;

pub fn unsupported() -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, "AirPlay is not available on this system")
}

pub struct UnixListener;
pub struct UnixStream;

impl UnixListener {
    pub fn bind(_path: &Path) -> io::Result<Self> {
        Err(unsupported())
    }

    pub fn incoming(&self) -> std::iter::Empty<io::Result<UnixStream>> {
        std::iter::empty()
    }
}

impl UnixStream {
    pub fn connect(_path: &Path) -> io::Result<Self> {
        Err(unsupported())
    }

    pub fn try_clone(&self) -> io::Result<Self> {
        Err(unsupported())
    }

    pub fn shutdown(&self, _how: std::net::Shutdown) -> io::Result<()> {
        Ok(())
    }
}

impl Read for UnixStream {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Err(unsupported())
    }
}
