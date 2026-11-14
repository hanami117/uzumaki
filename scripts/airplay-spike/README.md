# AirPlay spike (stages 1-2)

Sends Uzumaki's Apple Music audio to **one** AirPlay speaker. Off unless `UZUMAKI_AIRPLAY_SPIKE=1`;
without it the app, the bridge and the packages behave exactly as before. Plan and background:
`docs/research/airplay-sender.md`.

```
<audio> (MusicKit) ─ MediaElementSource ─┬─ local gain ─ speakers (0 while casting)
                                         └─ AudioWorklet: s16le 44.1 kHz stereo, 4096-frame chunks
preload ── IPC ──▶ bridge main.js ── Unix socket (UZUMAKI_AIRPLAY_PCM_SOCKET) ──▶ Rust (airplay/)
Rust: mDNS finds the speaker → runs cliairplay → PCM into its stdin, START on its command pipe
```

The tap is the player's own output, already decoded by Chromium. Nothing touches keys or the CDM,
nothing is written to disk, and MusicKit's volume is never changed (the local output is muted with a
gain node in the page's graph).

## Run it

```sh
scripts/airplay-spike/build-cliairplay.sh     # once: cliairplay v0.5.10 into target/cliairplay/
cargo run -p uzumaki-airplay -- list          # speakers on the network (browse only)
cargo run -p uzumaki-airplay -- plan Bedroom  # the cliairplay command the app would run (runs nothing)
scripts/airplay-spike/run.sh --dry-run        # no speaker: a stand-in prints the level it receives
scripts/airplay-spike/run.sh                  # the real thing (quit the normal Uzumaki first)
```

`run.sh` starts the test app (`packaging/macos/build.sh` output) or a `target/` build with the spike
on, and collects the app's, the helper's and the bridge's capture lines in one log
(`~/Library/Logs/Uzumaki/airplay-spike.log` on macOS, `~/.local/state/uzumaki/airplay-spike.log` on
Linux). Settings are environment variables, listed at the top of `run.sh` and `airplay/src/lib.rs`.

`uzumaki-cli` (the headless bridge client) also runs the spike when the variable is set.

## Log lines worth knowing

- `airplay capture: rms … dBFS …` (bridge, once a second while there is sound): the tap's level.
  `[silent]` while playing means the tap gets zeros.
- `airplay capture: element source: blob, crossOrigin unset, mediaKeys yes, playing` : a full song
  (MSE, Widevine). `audio-ssl.itunes.apple.com` is a signed-out preview, see below.
- `airplay: connected to "…"`, `bridge connected to the PCM socket`, `sent START`, `STARTED: …`:
  the session coming up. `ERROR <code>` is followed by what to do about it (`auth_required`: the
  HomePod's Home setting "Allow Speaker & TV Access").
- `clock ptp: stalled`: the speaker never answered our clock (firewall, UDP 319/320).

## Findings so far

- The capture graph works in castLabs Electron (AudioWorklet from a blob: URL is allowed by the page).
- **Signed-out previews are CORS-tainted:** they are plain files from another host and the element
  has no `crossOrigin`, so Chromium hands Web Audio zeros. That is CORS, not DRM. With the test
  switch `UZUMAKI_AIRPLAY_PREVIEW_CORS=1` (asks for CORS, which that CDN allows) previews measure
  about -16 dBFS in the tap, and the whole path (socket, Rust, helper stdin, START) was checked
  with the stand-in helper, local output muted. The switch takes effect from the second song (the
  first one starts loading before the element is tapped).
- Full songs (signed in) play from a same-origin `blob:` (MSE + Widevine), so they should not be
  tainted. Only a signed-in run can show it.
- The element's volume (MusicKit's volume) scales the tapped audio: the app's volume slider also sets
  the loudness on the speaker, on top of the speaker volume (`UZUMAKI_AIRPLAY_VOLUME`, default 30).
- Pause sends silence (the worklet keeps running), so the speaker session stays up; the speaker is
  about the helper's lead (~0.25-2 s) behind the app's clock and lyrics.

## Linux

The same code. Build the helper with the same script (`gcc`, `g++`, `make`, `git`). PTP timing
binds UDP 319/320, which needs `sudo setcap cap_net_bind_service=+ep target/cliairplay/cliairplay`;
without it the helper falls back to NTP timing. Let UDP 319/320 and the helper's ports through
`ufw`/`firewalld`. Not tried yet.
