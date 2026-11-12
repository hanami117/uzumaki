# Changelog

All notable changes to Uzumaki. Versions follow [semantic versioning](https://semver.org/); `packaging/release.sh`
takes each release's notes from its section here.

## [Unreleased]

### Added
- A build number beside the version (the `BUILD` file; 0.1.0 is build 1), so the packages can be rebuilt without
  code changes. It shows in About ("Version 0.1.0 (1)"), Settings → General → About and `uzumaki --version` /
  `uzu --version` ("0.1.0 (build 1)"), and is the macOS bundle's `CFBundleVersion`. The update check compares
  version, then build: a higher build of the same version is an update, and "Skip this version" skips one build.

## [0.1.0] - 2026-11-12

The first public release: a public beta for Linux and macOS. You need an Apple Music subscription. Uzumaki is not
affiliated with Apple.

### Apple Music
- Plays through Apple Music's own web player in a hidden castLabs Electron process (Widevine), at 256 kbps AAC, or
  64 kbps with Data saver. Lossless and Dolby Atmos are not available on the web.
- Guided sign-in in a small window, once. The library is kept on disk, so the app opens on it at once and works
  offline for browsing.
- Home, New, Radio, Made for You, Recently Added, Albums, Artists, Songs and Playlists, like Apple Music's sidebar,
  with pins, a collapsible rail and a narrow-window layout.
- Album, playlist and artist pages; the artist page has Apple's hero picture, Top Songs, Latest Release and sorted
  shelves with See All.
- Search in Apple Music and in your library as you type.
- Songs as a sortable table (title, artist, album, year, genre, time, date added) with columns that fit the width.
- Playlists: create, rename, add and remove songs, a picture of your own (shown in Uzumaki only); Add to Library;
  Favourites with stars.

### Playback
- Player bar (docked or floating), shuffle, repeat all and repeat one (seamless).
- Up Next with History, Play Next and Add to Queue, drag to reorder, smooth row motion.
- Full-screen Now Playing with synced lyrics, word by word where Apple has word timing.
- Resume where you left off: the last song, its position, Up Next and History come back paused at the next start
  (Settings → Playback, on by default). Nothing plays until you press Play.
- Multi-select with a right-click menu, drag and drop to playlists and the queue, Ctrl/Cmd+F to filter a list.

### Local files
- Music folders (FLAC, MP3, AAC/ALAC, Ogg Vorbis, WAV) played in-process, beside Apple Music or alone in Local
  files only mode (no Apple engine running).

### Desktop
- Linux: MPRIS media keys and panel widgets, tray icon, close to tray, desktop entry, song notifications.
- macOS: media keys and Control Center, menu bar icon and menus, Dock and Finder icon choice, `.app` bundle and DMG.
- Settings with search: themes (System, Light, Dark), app icon (Dark, Light, Coral, Follow theme), background
  pictures with effects (dither, ASCII, halftone, scanlines, blur), every keyboard shortcut remappable.
- Keyboard navigation throughout (Tab, arrows, Enter, Space).
- First-run onboarding with a shortcuts tour.
- `uzu`: an mpc-style command line and terminal player (queue, playlists, search, albums, lyrics, cover art) for a
  running Uzumaki; it starts the app hidden when needed.
- Script hooks on song and play-state changes.
- Update notice: once a day Uzumaki reads getuzumaki.app/latest.json (a plain request, nothing about you is sent)
  and says when a new version is out. Off switch in Settings → General. It never downloads anything itself.
  Check for Updates… in the macOS Uzumaki menu, the tray / menu bar icon and Settings → General (Check now) asks at
  once and always answers.
- Linux packages (AUR): the App icon choice writes user copies in `~/.local/share` and never touches `/usr`.

### Known limits
- macOS builds are signed ad hoc, not notarized: the first launch needs System Settings → Privacy & Security →
  Open Anyway.
- Music videos, concerts and radio shows on artist pages are not shown.

[0.1.0]: https://github.com/hanami117/uzumaki/releases/tag/v0.1.0
