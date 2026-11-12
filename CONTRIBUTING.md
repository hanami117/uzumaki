# Contributing to Uzumaki

Thank you for your interest. Bug reports, ideas and pull requests are welcome. For a bigger change, please
open an issue first, so we can agree on the approach before you spend time on it.

By contributing, you agree that your contribution is licensed under the project's licence,
`GPL-3.0-or-later`.

## Layout

| Folder | What it is |
|---|---|
| `app/` | The desktop app (`uzumaki`): Rust and [Rune](https://github.com/anomalyco/rune), pinned to one commit of Rune's `dev` branch. `app/src/ui/` has the views, `app/src/state/` the app state, `app/src/scenes.rs` the headless test scenes. |
| `bridge/` | The hidden player: castLabs Electron that loads music.apple.com and drives its MusicKit. See [docs/bridge-protocol.md](docs/bridge-protocol.md). |
| `ctl/` | `uzu`, the command line and terminal player. It talks to the app over a Unix socket; the protocol is in `ctl/src/lib.rs`. |
| `cli/` | A plain Rust test client for the bridge. |
| `fx/`, `fx-web/` | Background effects (dither, ASCII, halftone, scanlines, blur), and the same code as WebAssembly for the website's demo. |
| `packaging/` | `macos/build.sh` (app bundle and DMG), `release.sh` (prepares a release, publishes nothing), the AUR PKGBUILD and the Homebrew cask. |
| `site/` | The website, getuzumaki.app (static HTML). |
| `docs/` | [Rune API notes](docs/rune-notes.md), the [bridge protocol](docs/bridge-protocol.md), [hooks](docs/hooks.md). |

## Build and run

You need Rust (stable), Node.js with npm, git and pkg-config. On Linux also the ALSA development files and a
Vulkan driver.

```sh
cd bridge && npm ci && node node_modules/electron/install.js && cd ..   # once: castLabs Electron
cargo run -p uzumaki                                                    # the app
cargo run -p uzumaki-ctl --bin uzu                                      # uzu, against the running app
```

On macOS, castLabs Electron needs a Widevine VMP signature to stream (without it, Apple Music signs you in
and then shows "Something went wrong"). It is free:

```sh
python3 -m venv /tmp/evs && /tmp/evs/bin/pip install castlabs-evs
/tmp/evs/bin/python -m castlabs_evs.account signup                       # once
/tmp/evs/bin/python -m castlabs_evs.vmp sign-pkg bridge/node_modules/electron/dist
```

`packaging/macos/build.sh` then builds `Uzumaki.app` and a DMG in `target/macos.noindex/`. See castLabs'
[EVS wiki](https://github.com/castlabs/electron-releases/wiki/EVS) for details.

## Tests

```sh
cargo test --workspace
node --test bridge/
```

Neither needs Electron, a network or an Apple account.

## Check the UI without a window

The app can render a scene with made-up data to a PNG, with no Electron and no sound. This is the best way to
check a UI change:

```sh
cargo run -p uzumaki -- --list-scenes
cargo run -p uzumaki -- --snap /tmp/album.png --scene album --size 1360x860 --scale 1
```

`--tree` dumps the element tree instead.

## Test without touching your own login

A windowed run starts the hidden player, which can sign in and play sound. When you already use Uzumaki,
run a separate test copy with its own folders, so it can't change your login, settings, caches or volume:

```sh
t=/tmp/uzumaki-test; mkdir -p $t/config/uzumaki
echo '{"tray_icon": false}' > $t/config/uzumaki/settings.json
UZUMAKI_TEST_INSTANCE=1 UZUMAKI_MUTE=1 \
UZUMAKI_PROFILE_DIR=$t/profile XDG_CONFIG_HOME=$t/config XDG_CACHE_HOME=$t/cache \
TMPDIR=$t UZUMAKI_CTL_SOCKET=$t/uzu.sock \
cargo run -p uzumaki -- --hidden
```

- `UZUMAKI_TEST_INSTANCE=1`: no single-instance check, no MPRIS, no hooks, no notifications.
- `UZUMAKI_PROFILE_DIR`: a scratch profile for the hidden player, so your real Apple login isn't used.
- `UZUMAKI_MUTE=1`: the hidden player makes no sound.
- `UZUMAKI_CTL_SOCKET`: point `uzu` at the test copy with the same variable.

Don't change the volume from a test: Apple's player remembers it for your account.

## Rules

- Never try to decrypt Apple Music streams outside a real Widevine player. Uzumaki plays music only through
  Apple's own web player. Changes that get around DRM won't be accepted.
- Run `cargo test --workspace` before you open a pull request. Add a test or a headless scene for new
  behaviour where you can.
- New Rust dependencies must have a licence compatible with the GPL-3.0: `cargo deny check licenses`
  (with `deny.toml`) checks this.
- Keep personal data out of commits: no logins, cookies, profile folders or screenshots of a real library.
