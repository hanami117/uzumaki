//! Windows: media keys and the media overlay (System Media Transport Controls). The same
//! surface as `mpris.rs` and `nowplaying.rs`: `start(on_command)`, then `observe(&Event)`.

use crate::bridge::Event;
use crate::media::Command;

pub struct Smtc;

impl Smtc {
    pub fn start(_on_command: impl Fn(Command) + Send + Sync + 'static) -> Result<Self, String> {
        Err("not built yet".into())
    }

    pub fn observe(&self, _event: &Event) {}
}
