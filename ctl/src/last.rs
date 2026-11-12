//! The last numbered list `uzu` printed, so `uzu play <n>` means what the user saw. It is a small
//! JSON file next to the control socket ([`uzumaki_ctl::last_path`]). Every failure to read or
//! write it is ignored: a missing record just means "playlist n", as before.

use std::path::Path;

use serde::{Deserialize, Serialize};
use uzumaki_ctl::Kind;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "list", rename_all = "snake_case")]
pub enum Last {
    /// `playlists` / `albums`: the ids in printed numbering (a filter keeps the original numbers).
    Collections { kind: Kind, ids: Vec<String> },
    /// `playlist X` / `album X`: the songs of that collection.
    Songs { kind: Kind, id: String },
    /// `search`: the app keeps the results.
    Search,
    /// `queue`: the up-next song ids as printed.
    Queue { ids: Vec<String> },
}

pub fn save(path: &Path, last: &Last) {
    if let Ok(text) = serde_json::to_string(last) {
        let _ = std::fs::write(path, text);
    }
}

pub fn load(path: &Path) -> Option<Last> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_tolerates_garbage() {
        let dir = std::env::temp_dir().join(format!("uzu-last-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ctl.sock.last");
        assert_eq!(load(&path), None);
        for last in [
            Last::Collections { kind: Kind::Album, ids: vec!["l.1".into(), "l.2".into()] },
            Last::Songs { kind: Kind::Playlist, id: "p.x".into() },
            Last::Search,
            Last::Queue { ids: vec!["s1".into(), "s1".into()] },
        ] {
            save(&path, &last);
            assert_eq!(load(&path), Some(last));
        }
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(load(&path), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
