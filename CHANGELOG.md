# Changelog

All notable changes to Uzumaki. Versions follow [semantic versioning](https://semver.org/); `packaging/release.sh`
takes each release's notes from its section here.

## [Unreleased]

## [0.2.4] - 2026-11-18

A new About page in Settings, and a quiet way to support the project. You need an Apple Music subscription.

### Added
- **Support Uzumaki** (Settings → About): a quiet heart, one line and a "Support on Ko-fi" button that
  opens https://ko-fi.com/hanami117 in your browser. No popups, no reminders. Also in Settings search ("donate", "ko-fi").

### Changed
- **Settings has its own About page** (last in the list, after Keyboard), moved out of General: the app icon, the
  version and build, the copyright, the licences (GPL-3.0-or-later; castLabs Electron, symphonia, rodio, Rune and the
  fonts keep their own; not affiliated with Apple), links to getuzumaki.app and the source code, and the Support card.
  Settings search finds it ("version", "licence", "github", "donate").

## [0.2.3] - 2026-11-17

Show what you play on Discord (off by default), and fixes for forced themes on macOS and the `uzu` Queue screen. You need an Apple Music subscription.

### Added
- **Show on Discord** (Settings → General, off by default): Discord Rich Presence like Spotify's. Your profile shows
  "Listening to Uzumaki" with the song, the artist, the cover, a progress bar that follows seeks and pauses, and an
  "Open in Apple Music" button. Choices: show artwork, show buttons, hide when paused (on by default), hide the song
  title. Local files show title and artist only. It talks to the Discord app on the same computer through its local
  socket (including Flatpak and Snap Discord on Linux; Discord in a browser has none): Uzumaki opens no connection of
  its own, runs no thread while the switch is off, retries every 30 seconds while Discord isn't running, and clears
  the card when you pause, stop, switch it off or quit. Needs a Discord application id: see
  [docs/DISCORD.md](docs/DISCORD.md) (until the project's own is set, `discord.client_id` in settings.json).

### Fixed
- **A forced Light or Dark theme now reaches macOS's own parts of the window:** with Theme set to Light while the
  Mac is Dark (or the reverse), the traffic lights, the file picker and other system dialogs, tooltips and the menu
  bar menus used to stay in the Mac's appearance. They follow the Theme setting now, from the first moment the window
  shows. Theme = System still follows the Mac, and the “System (Light/Dark)” label still shows the Mac's own appearance.
- **`uzu`: the Queue screen no longer ends up in the History.** The played songs are folded under one "⟲ History · n songs"
  row (Enter on it, or `h`, unfolds them; remappable as `history` in `[keys]`), and the cursor follows the playing song
  when the next one starts, instead of staying with the old song as it slid up the History (after about 50 Next presses
  the list sat on the oldest song with the playing song and Up Next far below the screen). Unfolding keeps the playing song
  where it is on the screen with H1 right above it (k and the wheel go back through the latest played songs).

## [0.2.2] - 2026-11-16

AirPlay to several speakers at once, a fix for busy speakers, and starting at login. You need an Apple Music subscription.

### Added
- **Start Uzumaki when I log in** (Settings → General): on Linux an entry in your desktop's autostart programs
  (`~/.config/autostart`, it runs the AppImage, the package or your own install); on macOS a login item, listed in
  System Settings → General → Login Items. The switch shows what the system has, also after a change made there.
- **Start in the background:** a start at login opens no window. The music, media keys and the tray or menu bar icon
  are ready; show the window from the icon or by opening Uzumaki again.
- **AirPlay to several speakers at once (macOS)**, in sync like Apple's multi-room: the AirPlay picker has a
  checkbox next to each speaker; tick several (a stereo pair counts as one) and they play together. This Mac is
  either alone or off. Each speaker has its own volume slider, and with several a master slider moves them all.
  If one speaker is busy, off or drops out, the others keep playing and a message says which one stopped. Seek,
  pause and skip apply to all. `uzu airplay Bedroom + Living Room` does the same from the terminal, and
  “Reconnect to last AirPlay device” brings back the whole group.

### Fixed
- **AirPlay to a speaker that is playing from another device** (an iPhone playing to a HomePod, say): Uzumaki now
  takes it over the way Apple devices do, trying again after a short pause (up to three tries in about four
  seconds) instead of failing on the first refusal. If it still refuses, the message says “… is busy with another
  device” rather than asking you to check that it is on and on the same network.

## [0.2.1] - 2026-11-15

A fix for accounts that refuse explicit songs, and a Ko-fi link. You need an Apple Music subscription.

### Added
- **Apple's "E" badge** on explicit songs in song rows, Up Next, Now Playing and the player bar.
- A **Ko-fi** support link on getuzumaki.app and in the README, and GitHub's Sponsor button on the repository. The app
  itself is unchanged by it.

### Fixed
- **Explicit songs on an account that refuses them:** the player's refusal (`CONTENT_RESTRICTED`) is now explained
  instead of leaving playback stuck. Uzumaki says why once, marks explicit songs as unplayable (dimmed, passed over)
  and plays on from the next song that can play (after five refusals in a row it stops with one message).
- **The explicit content setting that counts is the web player's:** the message tells you to allow explicit content
  on music.apple.com and restart, with an "Open music.apple.com" button; Settings → Account → Explicit content
  explains that a Screen Time change on an iPhone or Mac may not reach the web player (try another browser if it
  keeps refusing). Accounts of a child under 18 can't turn it on.

### Site
- New page "Apple Music on Linux", a real 404 page, titles, descriptions and social cards per page, JSON-LD on the
  home page, and no layout shift in the home page's hero.

## [0.2.0] - 2026-11-14

AutoPlay, animated album artwork, AirPlay on the Mac, and a configurable `uzu`. You need an Apple Music subscription.

### Added
- **AutoPlay (∞):** when the queue ends, similar songs keep playing, as in Apple Music. Up Next (the side panel and
  Now Playing) has an ∞ button and an "AutoPlay" section with the upcoming suggestions, which you can play or remove;
  Now Playing's transport row has the button after repeat. It also continues after an album or playlist ends.
  Settings → Playback → AutoPlay turns it off (on by default); `uzu autoplay [on|off|toggle]` and the A key in `uzu`
  do the same.
- **Create Station** in the song menu and in an artist's menu, like Apple Music's.
- **Animated artwork:** albums with a motion cover (Apple's square loop) play it on the album page and in Now
  Playing, which now also slides and fades with the animation. Settings → Appearance → **Animated artwork**:
  Always, Now Playing only, or Off (Off downloads no clips). It follows Reduce Motion, pauses while the window is
  hidden, and Clear Caches removes the clips. On Linux it plays through `ffmpeg`, which must be installed.
- **Share → Copy Link** on albums, songs, playlists, artists and stations ("Link copied"). On macOS Share also lists the
  system's sharing services (AirDrop, Mail, Messages, …); on Linux the menu has Copy Link alone.
- **`uzu.toml`:** an optional config file for the `uzu` command line: `[theme]` (palettes and every colour of the
  screens), `[keys]` (remap any action), `[format]` (`status_format` for `uzu status --line`, for a status bar) and
  `[layout]` / `[defaults]` (start screen and view, columns, starting sorts, notifications). A bad line is a warning,
  never an error. `uzu config` checks the file and `uzu config --default` prints a commented one; every key is in
  `docs/UZU-CONFIG.md`.
- **AirPlay on macOS:** an AirPlay button in the player bar and in Now Playing picks a speaker (HomePod, Apple TV,
  stereo pairs, other AirPlay receivers), with a volume slider and password or PIN pairing, and the sound moves back
  to the Mac if the speaker goes away. Settings → Playback can reconnect to the last speaker; `uzu airplay` lists and
  picks speakers. Not available on Linux yet: there the button, the setting and the command are hidden.
- Cmd/Ctrl+L and Cmd/Ctrl+U open lyrics and Up Next in Now Playing; the mouse wheel over any volume control moves it
  in whole percents.

### Changed
- **Now Playing in a narrow window:** the volume has its own row under the scrubber, and the lyrics, Words, AirPlay,
  AutoPlay and Up Next buttons sit evenly spaced on a row of their own. Wide windows keep one row. Words is a round icon
  button like the others.
- Play and Shuffle from a menu (sidebar row, card) load the playlist's songs first and then play, so they start
  faster and a library playlist no longer fails with "not found".
- Pinned albums, playlists and stations have the full menu of what they pin; Up Next shows the favourite star.
- The sidebar rail lists every playlist.

### Fixed
- A song played from 0 after the restored (resumed) one had been replaced, instead of continuing at the old position.
- Lyrics requests that failed are tried again, and "no lyrics" expires instead of sticking for good.
- A dropped sidebar pin glides home on one smooth movement, without a bump.
- Local files with no album tag show the song's initial on the cover tile instead of their folder's.
- Up Next rows use the dark colours on the Light theme.
- Linux: no AirPlay row or command that can't work.

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

[0.2.0]: https://github.com/hanami117/uzumaki/releases/tag/v0.2.0
[0.1.1]: https://github.com/hanami117/uzumaki/releases/tag/v0.1.1
[0.1.0]: https://github.com/hanami117/uzumaki/releases/tag/v0.1.0
