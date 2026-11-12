# The playback bridge

`bridge/` is a small castLabs Electron app (castLabs ships Widevine) that loads music.apple.com and drives the
page's own player, `MusicKit.getInstance()`. The Rust app starts it as a child process and talks to it with
newline-delimited JSON over stdin/stdout. `cli/` is a plain Rust test client for it.

## Layout

```
bridge/   castLabs Electron (v45.2.1+wvcus) loading music.apple.com, JSON over stdin/stdout
cli/      Rust test client that spawns the bridge and takes typed commands
```

## Setup (once)

```sh
cd bridge
npm install                          # .npmrc allows the castLabs git dependency (npm 12 blocks git deps by default)
node node_modules/electron/install.js  # downloads the Electron binary (otherwise happens on first launch)
```

The Widevine CDM downloads itself on first launch into `~/.config/uzumaki/bridge/`
(login cookies live there too).

## Run

```sh
cd cli
cargo run              # add -- --show to start with the window visible, -- --verbose for time ticks
```

Then: `login` → sign in to Apple Music in the window → `hide` → `search <words>` → `play 1`.
Type `help` for all commands. Electron's stderr goes to `/tmp/uzumaki-bridge.log`.

## Bridge protocol (newline-delimited JSON)

Commands (stdin), each with an optional numeric `id` that is echoed in the reply:

| cmd        | fields                     |                                         |
|------------|----------------------------|-----------------------------------------|
| `play` `pause` `next` `previous` | | transport                         |
| `seek`     | `time` (seconds)           |                                         |
| `volume`   | `value` (0..1)             |                                         |
| `setQueue` | one of `songIds` [string], `album`, `playlist` (catalog or library id); `startWith` (index), `startPlaying` (default true) | |
| `search`   | `term`, `limit`            | → `{songs: [{id,title,artist,album,duration}]}` |
| `api`      | `path`, `params`, optional `method`/`body` | Apple Music API via the page; `{{storefrontId}}` is substituted; a `body` makes it a write (e.g. add to playlist) |
| `status`   |                            | → `{authorized, storefront, state, nowPlaying, current, duration, queueLength, volume}` |
| `setUpcoming` | `songIds`, `loop` (repeat all: the next lap's order, as Up Next shows it, or null) | what plays after the current song; the app owns the order |
| `setRepeat` | `mode`: `off` / `one` / `all` | |
| `timeRate` | `hz` (1–10) | time events per second (4 while synced lyrics show) |
| `widevine` |                            | → `{available}` (EME check)             |
| `setBitrate` | `kbps`: 256 or 64        | streaming quality; MusicKit resets it to 256 on every page load, so the app re-sends it after `ready` |
| `signOut`  |                            | stops playback, `mk.unauthorize()`, removes the profile's apple.com cookies and music.apple.com storage, reloads the page → `{cookiesRemoved}` |
| `signIn`   | optional `near`: `{x, y, w, h}`, our window's frame in screen points | the guided sign-in: the window at about 520×740, centred on `near` (macOS) or on the screen (Linux, where it is a dialog-type window so tiling WMs float it), titled "Sign in to Apple Music", then Apple's sign-in; again while it shows: brings it to the front |
| `show` `hide` `reload` `quit` |         | window / process control (`show`: the full 1100×800 window) |

Replies (stdout): `{"id":1,"ok":true,"result":...}` or `{"id":1,"ok":false,"error":"..."}`.

Events (stdout):
`{"event":"widevine","ok":true,"status":{...}}`,
`{"event":"ready","authorized":bool,"storefront":"us"}` (MusicKit found; re-sent after page reloads),
`{"event":"auth","authorized":bool,"storefront":"it"}` (`storefront`: MusicKit's at that moment),
`{"event":"storefront","id":"it"}` (MusicKit switched storefront, e.g. to the account's after sign-in),
`{"event":"nowPlaying","item":{id,catalogId,title,artist,album,duration,artwork,artworkBg,hasLyrics}}`,
`{"event":"playbackState","state":"playing|paused|loading|waiting|stopped|ended|..."}`,
`{"event":"time","current":s,"duration":s}` (1/s, or `timeRate`),
`{"event":"volume","value":0..1}`, `{"event":"window","visible":bool,"reason":"closed|hidden"}` (`reason` on a hide
only: `closed` when the user closed the window, `hidden` when the app asked or signed in), `{"event":"error","message":"..."}`.

While the window shows for `signIn`, a small CSS layer (`bridge/signin.js`, applied by `preload.js` in
music.apple.com's main frame only) fades the page around Apple's sign-in dialog into Uzumaki's background. It never
touches the dialog (a cross-origin frame) or reads from it, and does nothing if it can't find it. No message carries
credentials. `node --test bridge/` checks the window maths and the event (no Electron, no network).

Closing the bridge's stdin makes it quit.

