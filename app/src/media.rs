//! What the desktop's media controls ask for, on every system: MPRIS on Linux (`mpris.rs`),
//! MediaPlayer.framework on macOS (`nowplaying.rs`), the System Media Transport Controls on
//! Windows (`smtc.rs`). `main.rs` routes the commands to the app state.

/// Which copy already runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Running {
    Current,
    /// A build from before the rename (still called Cadence). Its data must not be moved
    /// while it runs.
    #[cfg_attr(not(unix), allow(dead_code))]
    Legacy,
}

/// What the desktop asked for. `main.rs` routes these to the bridge / app handle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    Play,
    Pause,
    Next,
    Previous,
    /// Absolute position, seconds.
    SeekTo(f64),
    /// 0..1
    #[cfg_attr(windows, allow(dead_code))]
    Volume(f64),
    #[cfg_attr(windows, allow(dead_code))]
    Raise,
    #[cfg_attr(windows, allow(dead_code))]
    Quit,
}
