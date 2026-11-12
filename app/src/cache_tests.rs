use super::*;
use crate::models::CollectionKind;

fn temp(name: &str) -> Cache {
    let dir = std::env::temp_dir().join(format!("uzumaki-cache-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    Cache::at(dir)
}

fn collection(id: &str) -> Collection {
    Collection {
        kind: CollectionKind::Playlist,
        id: id.into(),
        title: "T".into(),
        subtitle: "S".into(),
        artwork: None,
        year: None,
        genre: None,
        description: None,
        track_count: Some(1),
        modified: Some("2026-01-01T00:00:00Z".into()),
        editable: false,
        favorites: false,
        public: None,
        tracks: Vec::new(),
        artists: Vec::new(),
    }
}

#[test]
fn round_trips_through_disk() {
    let c = temp("roundtrip");
    c.write(PLAYLISTS, &vec![collection("p.1"), collection("p.2")]).unwrap();
    let back: Stored<Vec<Collection>> = c.read(PLAYLISTS).unwrap();
    assert_eq!(back.data, vec![collection("p.1"), collection("p.2")]);
    assert!(back.saved_at > 0);
    c.write(&Cache::collection_name("p.1"), &collection("p.1")).unwrap();
    assert_eq!(c.read_collection("p.1").unwrap().data.id, "p.1");
    assert!(c.read_collection("p.9").is_none());
    let all = c.load_all();
    assert_eq!(all.playlists.unwrap().len(), 2);
    assert!(all.songs.is_none() && !all.authorized);
    let _ = std::fs::remove_dir_all(&c.dir);
}

#[test]
fn wrong_version_or_garbage_is_no_cache() {
    let c = temp("version");
    std::fs::create_dir_all(&c.dir).unwrap();
    std::fs::write(c.dir.join(ALBUMS), r#"{"version": 999, "saved_at": 1, "data": []}"#).unwrap();
    std::fs::write(c.dir.join(SONGS), "{not json").unwrap();
    std::fs::write(c.dir.join(PLAYLISTS), format!(r#"{{"version": {VERSION}, "saved_at": 1, "data": "wrong shape"}}"#)).unwrap();
    assert!(c.read::<Vec<Collection>>(ALBUMS).is_none());
    assert!(c.read::<Vec<Song>>(SONGS).is_none());
    assert!(c.read::<Vec<Collection>>(PLAYLISTS).is_none());
    let all = c.load_all();
    assert!(all.albums.is_none() && all.songs.is_none() && all.playlists.is_none());
    let _ = std::fs::remove_dir_all(&c.dir);
}

#[test]
fn collection_names_are_sanitized_and_old_files_pruned() {
    assert_eq!(Cache::collection_name("../a/b c"), "collections/.._a_b_c.json");
    let c = temp("prune");
    for i in 0..5 {
        c.write(&Cache::collection_name(&format!("{i}")), &collection(&i.to_string())).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    c.prune_collections(3);
    assert!(c.read_collection("0").is_none() && c.read_collection("1").is_none());
    assert!(c.read_collection("2").is_some() && c.read_collection("4").is_some());
    let _ = std::fs::remove_dir_all(&c.dir);
}

#[test]
fn newest_ids_must_match_in_order() {
    let probe: Vec<String> = ["c", "b"].iter().map(|s| s.to_string()).collect();
    assert!(same_newest(["c", "b", "a"].into_iter(), &probe));
    assert!(!same_newest(["b", "c", "a"].into_iter(), &probe));
    assert!(!same_newest(["c"].into_iter(), &probe));
}

#[test]
fn playlist_is_current_only_with_equal_known_dates() {
    assert!(playlist_current(Some("x"), Some("x")));
    assert!(!playlist_current(Some("x"), Some("y")));
    assert!(!playlist_current(None, None));
    assert!(!playlist_current(Some("x"), None));
}

#[test]
fn fresh_only_with_no_session_and_no_library() {
    let c = temp("fresh");
    assert!(c.fresh(), "nothing on disk: a new install");
    c.write(SESSION, &false).unwrap();
    assert!(!c.fresh(), "signed out on record: ran here before");
    let c = temp("fresh-lib");
    c.write(ALBUMS, &Vec::<Collection>::new()).unwrap();
    assert!(!c.fresh(), "a library on disk");
    let c = temp("fresh-in");
    c.write(SESSION, &true).unwrap();
    assert!(!c.fresh() && c.signed_in());
}
