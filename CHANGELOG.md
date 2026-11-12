# Changelog

All notable changes to Uzumaki. Versions follow [semantic versioning](https://semver.org/); `packaging/release.sh`
takes each release's notes from its section here.

## [Unreleased]

## [0.1.1] - 2026-11-12

Linux packages, a fixed sign-in and store, and a few fixes. You need an Apple Music subscription.

### Added
- **Linux packages:** an AppImage for any x86_64 distribution with glibc 2.35 or newer (Ubuntu 22.04+, Debian 12+,
  Fedora 36+), and two AUR packages: `uzumaki` (builds from source) and `uzumaki-bin` (the AppImage's files,
  nothing to build). The AppImage carries update information for AppImageUpdate, and `./Uzumaki-x86_64.AppImage uzu …`
  runs the `uzu` command line. Inside the AppImage the playback engine runs without Chromium's sandbox (an AppImage
  can't carry the setuid helper it needs); the packages keep the sandbox.
- **Text fields show where you type:** a coral caret that blinks while the field has the keyboard, and a coral
  selection, in Search, Settings search, the Ctrl/Cmd+F filter and the playlist name fields. Before, on the Light
  theme neither was visible, so Ctrl/Cmd+A showed nothing.
- **A build number** beside the version, so a package can be rebuilt without code changes: About shows
  "Version 0.1.1 (1)", `uzumaki --version` "0.1.1 (build 1)". The update check compares version, then build.
- **An anonymous count of update checks.** The daily check now asks `updates.getuzumaki.app` and sends only the
  Uzumaki version, the OS, the processor type and how it was installed (AppImage, AUR, Homebrew, DMG, …), plus a flag
  on the first check of a week and of a month. The server keeps daily totals and nothing else: no IP address, no id,
  no cookies (see Privacy in the README; the server code is in `server/update`). Turning the check off in
  Settings → General sends nothing.
- **"Update required" warning:** a version known not to work with Apple Music any more shows a warning that can't
  be closed or skipped, with the download or the package manager's command. The app keeps running.

### Changed
- Package installs say how to update instead of offering a download: the AUR package "yay -Syu uzumaki", the
  Homebrew cask "brew update && brew upgrade --cask uzumaki", other packages "Update with your package manager".
- The update check runs once per UTC day, and a failed check is tried again an hour later instead of the next day.
- **The store is your Apple account's:** Uzumaki used the US store for every account (music.apple.com's default),
  so New, Radio, artist pages and search showed US content and some songs played only as previews. It now asks
  Apple for the account's store and switches to it; pages already loaded load again.

### Fixed
- Signing in didn't work once the sign-in window had been hidden before (for example, closed and opened again).
- Sign out now clears everything of the account: the library could come back after signing out (replies asked
  before it arrived later), and artists, Recently Added, Made for You, your station, pins, the resumed song and Apple
  songs in History stayed. Local files stay.
- Sidebar pins: a dropped pin settles smoothly into its place from where you let it go (it jumped back up first),
  and the other pins slide aside while you drag instead of jumping.
- AppImage: the App icon setting doesn't write desktop overrides that would point at a binary that isn't there.

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

[0.1.1]: https://github.com/hanami117/uzumaki/releases/tag/v0.1.1
[0.1.0]: https://github.com/hanami117/uzumaki/releases/tag/v0.1.0
