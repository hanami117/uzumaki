//! Windows backend: a toast notification (WinRT).

use super::{Backend, Note};

pub(super) struct Sys;

impl Sys {
    pub fn new() -> Self {
        Sys
    }
}

impl Backend for Sys {
    fn send(&self, _note: &Note) {}
}
