//! macOS: Rune's own `Player` (AVPlayer + `AVPlayerItemVideoOutput`): hardware decode,
//! zero-copy IOSurface frames, looping, muted. Its first frame (poster) is decoded up front
//! (`preload`), so the layer fades in over the still with the clip's first picture.

use super::Clip;

pub struct Native {
    player: rune::Player,
}

impl Native {
    pub fn start(clip: &Clip) -> Self {
        let player = rune::Player::file(clip.path.clone());
        player.set_muted(true);
        player.set_looping(true);
        player.preload();
        player.play();
        Self { player }
    }

    pub fn player(&self) -> &rune::Player {
        &self.player
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
