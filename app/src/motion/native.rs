//! macOS: Rune's own `Player` (AVPlayer + `AVPlayerItemVideoOutput`): hardware decode,
//! zero-copy IOSurface frames, looping, muted.
//!
//! In a window the clip doesn't count as shown until the GPU has drawn a real frame of it:
//! it starts paused with its first frame (poster) decoded off the main thread (`preload`), its
//! layer is drawn nearly transparent so Rune pulls that frame, and the surface's observer
//! reports the first frame drawn. Only then does the layer fade in over the still and the clip
//! play, so the crossfade always starts from a picture, never from an empty box (an AVPlayer
//! has no frame for a moment after it starts). Headless runs decode each frame synchronously
//! as they draw, so there the clip shows and plays at once.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::Clip;

pub struct Native {
    player: rune::Player,
    /// A real frame of the clip has been drawn.
    seen: Arc<AtomicBool>,
    started: std::time::Instant,
}

/// If no drawn frame is reported by then (a surface Rune never got to draw), the clip shows and
/// plays anyway, as it did before it waited: never stuck on the still.
pub const PATIENCE: std::time::Duration = std::time::Duration::from_millis(1500);

impl Native {
    /// `wait`: hold the clip paused on its poster until a frame is drawn (`on_frame` hears it,
    /// from the render thread); else play at once.
    pub fn start(clip: &Clip, wait: bool, on_frame: Box<dyn Fn() + Send + Sync>) -> Self {
        let player = rune::Player::file(clip.path.clone());
        player.set_muted(true);
        player.set_looping(true);
        let seen = Arc::new(AtomicBool::new(!wait));
        if wait {
            let flag = seen.clone();
            player.surface().set_observer(Arc::new(move |_| {
                if !flag.swap(true, Ordering::AcqRel) {
                    on_frame();
                }
            }));
            player.preload();
        } else {
            player.preload();
            player.play();
        }
        Self { player, seen, started: std::time::Instant::now() }
    }

    pub fn player(&self) -> &rune::Player {
        &self.player
    }

    /// A real frame has been drawn (always, when not waiting), or `PATIENCE` ran out.
    pub fn has_frame(&self) -> bool {
        self.seen.load(Ordering::Acquire) || self.started.elapsed() >= PATIENCE
    }

    pub fn set_paused(&self, paused: bool) {
        if paused { self.player.pause() } else { self.player.play() }
    }

    pub fn failed(&self) -> bool {
        matches!(self.player.state(), rune::PlayerState::Failed(_))
    }
}

impl Drop for Native {
    fn drop(&mut self) {
        // Dropping the last clone (and the element showing it) frees the player; pause first
        // so nothing decodes in between.
        self.player.pause();
    }
}
