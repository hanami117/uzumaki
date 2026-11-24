//! `.m3u8` export and import (`local::m3u`): menu items ask for a file dialog here, and the
//! next `poll` (which has a `Tx`) opens it.

use rune::Tx;

use crate::models::Collection;
use crate::state::State;

/// A file dialog a menu item asked for.
pub enum FileAsk {
    /// Save the local songs of this playlist.
    Export(Collection),
    /// Pick a `.m3u8` to make an Uzumaki playlist of.
    Import,
}

/// Opens the dialog a menu item asked for. Never redraws by itself.
pub fn poll(s: &mut State, tx: &mut Tx) -> bool {
    match s.ui.file_ask.take() {
        Some(FileAsk::Export(c)) => {
            let opts = rune::FileDialogOptions { title: format!("Export “{}”", c.title), file_name: Some(format!("{}.m3u8", safe_name(&c.title))), ..Default::default() }
                .filter("Playlist", &["m3u8", "m3u"]);
            tx.save_file(opts, move |s: &mut State, result: rune::FileDialogResult, _tx: &mut Tx| match result {
                Ok(Some(mut path)) => {
                    if path.extension().is_none() {
                        path.set_extension("m3u8");
                    }
                    s.export_m3u8(&c, &path);
                }
                Ok(None) => {}
                Err(e) => s.toast = Some(format!("Couldn't open the save dialog: {e}")),
            });
        }
        Some(FileAsk::Import) => {
            let opts = rune::FileDialogOptions { title: "Import a playlist".into(), ..Default::default() }.filter("Playlist", &["m3u8", "m3u"]);
            tx.open_file(opts, |s: &mut State, result: rune::FileDialogResult, _tx: &mut Tx| match result {
                Ok(Some(path)) => {
                    s.import_m3u8(&path);
                }
                Ok(None) => {}
                Err(e) => s.toast = Some(format!("Couldn't open the file picker: {e}")),
            });
        }
        None => {}
    }
    false
}

/// A playlist name as a file name: no path separators or characters Windows refuses.
fn safe_name(title: &str) -> String {
    let s: String = title.chars().map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '_' } else { c }).collect();
    let s = s.trim().trim_matches('.').to_string();
    if s.is_empty() { "Playlist".into() } else { s }
}

#[cfg(test)]
mod tests {
    #[test]
    fn file_names_are_safe() {
        assert_eq!(super::safe_name("AC/DC: Best?"), "AC_DC_ Best_");
        assert_eq!(super::safe_name(" .. "), "Playlist");
    }
}
