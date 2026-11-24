# Changelog

All notable changes to Uzumaki. Versions follow [semantic versioning](https://semver.org/); `packaging/release.sh`
takes each release's notes from its section here.

## [Unreleased]

### Added
- **Local files: lyrics from [LRCLIB](https://lrclib.net)** for songs that have none of their own (no `.lrc`,
  `.txt` or lyrics tag). Synced lyrics when LRCLIB has them, else plain ones; songs marked instrumental get none.
  They show in Now Playing, the side panel and uzu's lyrics screen with a small "Lyrics from LRCLIB". Each song is
  asked once: answers (and "not found", asked again after ten days) are kept in Uzumaki's cache. Sends the song's
  title, artist, album and length to lrclib.net; on by default, off in Settings → Local Files → Lyrics. Offline or
  when LRCLIB is busy nothing breaks: it waits (honouring LRCLIB's Retry-After) and asks again later.
- **Settings → Local Files → Lyrics:** "Save .lrc files next to the songs" (off by default; `.txt` for lyrics that
  aren't time-synced, never over an existing file) and "Download lyrics for all local songs", which asks LRCLIB
  about every song without lyrics, one at a time, with progress and Stop.

## [0.4.3] - 2026-11-24

More for your own music files (favourites, lyrics, better albums, playlist import and export), Chromecast plays local
files on the TV itself and is no longer Beta, and the system's media controls always show what the window shows. You
need an Apple Music subscription to stream; local files play without one.

### Added
- **Local files:** favourites. Star your own songs anywhere (song rows, menus, Now Playing), signed in or not.
  They are kept on this computer only (`local-favourites.json` beside `playlists.json`) and never sent to Apple.
  They show at the end of Apple's Favourite Songs, or on their own Favourite Songs page in local files only mode.
- **Local files:** lyrics. A `.lrc` beside the file (synced), the file's own lyrics tags (ID3 `SYLT`/`USLT`,
  Vorbis `LYRICS`, MP4), or a `.txt` beside it, in Now Playing, the side panel and uzu's lyrics screen.
- **Playlists:** export an Uzumaki playlist, the local Favourite Songs or the local songs of a mixed playlist as
  `.m3u8`, and import a `.m3u8` as a new Uzumaki playlist (right-click the + beside Playlists). Only files in your
  music folders are imported.

### Changed
- **Local files:** better albums. A compilation flag without an album artist makes the album "Various Artists";
  an album without album artist tags stays one album (by the artist most of its songs share, else Various
  Artists) instead of one per artist; track and disc numbers like `3/12` are read; a disc folder (`CD1`,
  `Disc 2`) takes the cover beside it in the album folder, and covers from any of a file's tags count. Your
  music folders are read once again after the update to pick these up.
- **Chromecast is no longer Beta** (still off by default: Settings → Speakers → Chromecast; macOS and Linux).
- **Chromecast: local files are played by the TV itself** when it can (MP3, AAC, FLAC up to 96 kHz/24-bit, Ogg,
  WAV): no delay behind the app, seeks are the TV's own, the file's own quality. Other files (ALAC, AIFF, …) and
  Apple Music still go as a live stream. Mixed queues switch between the two by themselves.
- **Chromecast: a song that follows on by itself shows its own title, artist and cover on the TV.** The TV first
  plays out the old song, then starts the new one from its first note, after a short pause.

### Fixed
- The system's media controls (media keys, the Linux panel's player widget, macOS Control Center, the Windows media
  overlay) now show exactly what the window shows: the new song while it loads (not the old one paused), a song
  resumed at startup, the stop after the last unavailable song, and on a Chromecast the position you hear. The tray's
  Play/Pause follows the same state.
- Pause pressed while a song is still loading now shows Paused instead of the loading spinner.
- Previous on a Chromecast counts the position you hear: 1 s into a song it goes back a song instead of restarting it.
- Local files follow the system's audio output: plugging in headphones or switching AirPods moves the song there
  where it was. When the output device goes away, the song moves to the default one, or pauses (the bar no longer
  stays on Playing at a frozen time); Play tries again.
- **uzu:** `uzu pause` and `uzu vol` (no value) no longer start Uzumaki just to find nothing playing.
- **Local files:** a music folder on a drive that isn't connected keeps its songs (and their places in your
  playlists) instead of dropping them; Settings → Local files says "Not connected".
- **Mixed playlists:** a song found unavailable shows greyed in the merged list at once.
- **Chromecast: a lost connection comes back.** TV switched off and on, Wi-Fi dropping or the computer waking from
  sleep: Uzumaki pauses where you stopped hearing the music, tries again for a minute and plays on. If the TV doesn't
  come back, the music waits, paused, on this computer at that spot.
- **Chromecast: back to this computer from where you were** (the picker, `uzu cast off`, or the TV taken by another
  app), not a couple of seconds further on, as AirPlay does.

## [0.4.2] - 2026-11-23

Fixes from a review of `uzu` and the app: the terminal player acts on the song you saw, settings and playlists files
survive a bad value or a read error, and playback recovers when the engine crashes. You need an Apple Music
subscription to stream; local files play without one.

### Fixed
- **uzu:** the Queue screen kept showing an old list when a filter (`/`) was on and another playlist started from
  the window. A queue from another source now clears the filter and says so. With this app version the screen also
  follows every queue change within half a second, instead of every 3 s.
- **uzu:** the Queue filter is also cleared when a new queue has the same name as the old one (two searches played
  from the window, two playlists with the same title).
- **uzu:** commands and keys that pick a song by its number now act on the song you saw, even if the list changed
  in between (a song ended, a song was added to the playlist, another `uzu` searched): `uzu del`/`jump`/`move`/`play
  <n>`, `uzu pick`, Enter, `d`, `J`/`K` in the TUI. If that song is gone, uzu says so instead of picking another one.
- **uzu:** Enter on an AutoPlay suggestion in the Queue screen plays it, instead of an "Up Next has N songs" error.
- **uzu:** `J`/`K` keep the selection on the moved song, so pressing `J` again moves the same song further.
- **uzu:** lists written to a pipe or a file (`uzu playlists | grep …`, `uzu queue -a > file`) keep whole titles
  instead of cutting them at 80 columns; the search hint and the "from the cache" note stay out of them.
- **uzu:** the Lyrics screen works again after Uzumaki was updated and restarted while `uzu` stayed open, and a reply
  `uzu` can't read says to update `uzu` instead of "lost the connection". `start_screen` in `uzu.toml` takes a number
  in quotes and says "1 to 5" for one out of range.
- A settings file with one value this version doesn't know (written by a newer Uzumaki, for example) no longer resets
  every setting and shows the onboarding again: only that value goes back to its default, and the original file is
  kept as `settings.json.bak`.
- A `playlists.json` that can't be read (permissions, a disk error) is no longer taken for a first run and replaced
  at the next change: the app says so and leaves the file alone.
- When the playback engine (Electron) crashes, the player no longer keeps saying Playing: the song pauses where it
  was, and the engine starts again by itself (a Restart button appears if it keeps crashing). A crashed Apple Music
  page now restarts the engine too, instead of leaving playback dead until the app was restarted.
- A local file deleted, renamed or on a drive that's gone while it waits in the queue is skipped, instead of
  stopping the queue. Greyed songs (missing files, unavailable songs) are passed over after a local file too.
- Previous on a song added with Play Next goes back to the song you heard before it.
- Library pages, Home and the browse pages that failed to load (offline at launch, after waking the computer) load
  again when you open them, come back to the window, or the network returns.
- Playlists edited on another device show their changes while the app keeps running (checked every 15 minutes), and
  local songs in them stay merged.
- Covers that failed to load while offline load again later instead of staying grey until a restart.
- Chromecast: `uzu status`, Discord and keyboard seeks follow the position you hear, like the window.
- macOS and Linux: two launches at the same moment no longer start two copies of the app.

## [0.4.1] - 2026-11-23

Mixed playlists (Apple Music songs and your own files together), AirPlay on Linux, and JPEG XL pictures. You need
an Apple Music subscription to stream; local files play without one.

### Added
- **Mixed playlists.** "Add to Playlist" now works on local files (the song menu, Local Files, several songs at
  once, or a drag onto a playlist). In your Apple Music playlists they are kept on this computer only, marked with
  a small icon: Apple Music can't hold your own files and nothing is sent to it. They keep their place next to the
  songs around them when the playlist changes on your iPhone or Mac. Remove from Playlist takes them out again, and
  Move Up / Move Down moves them. A file you moved or renamed is found again by its tags; one that is gone shows
  greyed as "File not found".
- **Uzumaki playlists:** New Playlist can make a playlist kept in Uzumaki on this computer ("This computer"), which
  holds Apple Music songs and local files together. It sits in the sidebar with its own icon; rename it, give it a
  picture, reorder its songs or delete it from its menu. It also works in local files only mode.
- Uzumaki playlists now also show on the All Playlists page, after your Apple Music ones, with the same small icon as
  in the sidebar ("On this computer") and the same card menu (play, edit for name and picture, pin, delete). That
  also works signed out and in local files only mode.
- `uzu playlist add-local <playlist> <file>`, and `uzu playlist` marks local songs ("local", or "missing").
- **AirPlay on Linux:** the AUR packages (`uzumaki` and `uzumaki-bin`), the AppImage and `install.sh` now include
  the AirPlay helper, so HomePods and other AirPlay speakers, one or several in sync, work as on macOS. Speakers
  are found with mDNS: a VPN kill switch or a firewall must let multicast to `224.0.0.251` (UDP 5353) through
  (see "AirPlay on Linux" in the README). When no speaker shows up after a few seconds, the AirPlay menu says so.
- **JPEG XL pictures.** Playlist pictures (New Playlist, Choose Picture…) and the Appearance background picture
  now accept `.jxl` files besides PNG and JPEG. A JPEG XL is converted like any other picture (an animated one gives
  its first frame), so nothing is stored as JPEG XL.

### Fixed
- **AirPlay and Chromecast:** songs you uploaded to your iCloud Music Library (not in Apple's catalog) were silent on
  the speaker. Their audio comes from another Apple server, and the browser engine hid it from Uzumaki; it is now
  read like the others. If that ever fails, the song still plays on this computer and a message says why the
  speaker is silent.
- **AirPlay:** going back to This Computer, the song goes on from where you were listening, instead of about a
  second further on (the speakers play a little behind the app; the app now steps back by their actual delay),
  and without a stutter: this computer stays silent until the player is there, then fades in.
- **Discord:** with "Hide when paused" off, a paused song's card showed a time that kept running past the song's
  length (Discord counts on its own under a card without a bar). The card now shows the bar held still at 0:00
  while paused (Discord can't show a bar stopped where you paused); playing again picks up from the right place.

## [0.4.0] - 2026-11-20

Reorganised Settings and, as a Beta, Chromecast. You need an Apple Music subscription.

### Added
- **Chromecast (Beta, off by default):** Settings → Speakers → Chromecast. Google TVs and Cast speakers on your
  network then show up in the speaker menu, with their volume; the player bar shows where the sound goes. The
  sound arrives a few seconds behind the app. macOS and Linux; not on Windows yet. `uzu cast` controls it too.

### Changed
- **Settings, reorganised:** 11 shorter pages instead of 7: General, Account, Playback, **Speakers**, Appearance,
  **Now Playing**, **Notifications** (song notifications and Discord), Local Files, Keyboard, **Advanced** (updates,
  the welcome tour, storage) and About. Rows that only matter when another switch is on hide until it is. Search
  finds the old names too. Nothing was removed and your settings stay as they are.

### Fixed
- **Discord:** with Title – Artist, the album no longer shows twice on your card.
- An install with a missing player (castLabs Electron) now says the install is incomplete, instead of naming a
  folder from the machine that built it.

## [0.3.1] - 2026-11-18

A small update for Discord. You need an Apple Music subscription.

### Added
- **Discord: choose what the status line shows** (Settings → General → Show in status, right below Show on
  Discord): Title – Artist (the default), Song title, Artist, or App name. With Title – Artist the album
  moves to the second line of your activity. The change applies right away.

## [0.3.0] - 2026-11-18

Uzumaki now runs on Windows. You need an Apple Music subscription.

### Added
- **Windows 10 and 11 (x64):** an installer for your account only (no administrator rights, a Start menu entry,
  `uzu` on your PATH, an uninstaller) and a zip. Apple Music plays with Widevine as on the other systems, and your own
  music files play too. Media keys and the Windows media overlay, song notifications, the tray icon, start at login,
  Discord and `uzu` (over a named pipe) all work. The update notice offers the new installer.
  Not on Windows: AirPlay and script hooks. The app is not code-signed yet: if SmartScreen stops it, click
  "More info", then "Run anyway".

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
