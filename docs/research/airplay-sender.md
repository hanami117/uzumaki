# AirPlay sender inside Uzumaki: research and plan

Date: 2026-11-14. Research only. This note changes no app code.
It follows `airplay-macos.md` (same branch). That note looked at the OS output only.

## What changed

New rule (2026-11-14): Uzumaki may take the audio **at the output of the player**. This is the audio that the
Widevine player in Chromium has already decrypted and decoded, the same samples that go to the speakers. Uzumaki may
send that audio to the user's own devices (AirPlay now, maybe Chromecast later), as Cider does.

Still forbidden: getting keys or the CDM, decrypting outside the Widevine player, saving audio to files, and
sending audio to anyone else.

## Verdict

**Yes, this is feasible on macOS and on Linux, with the same code.** We need no change to the system output.

- **Capture:** Web Audio on MusicKit's `<audio>` element (`createMediaElementSource` → `AudioWorklet`) gives real
  PCM for Widevine audio in castLabs Electron. Chromium sends zeros only for CORS taint, not for encrypted media.
  Cider 1 ships this exact path (section 2).
- **Send:** do not write an AirPlay stack ourselves. Run **`cliairplay`** (Music Assistant, GPL-3.0, C, macOS +
  Linux) as a helper process, one per speaker. It speaks AirPlay 1 (RAOP) and AirPlay 2 (transient pairing, PTP or
  NTP). It reads raw PCM on stdin, and it starts the audio at an exact wall-clock time. That fits our model: Rust
  already runs Electron as a child process.
- **Discovery:** the `mdns-sd` crate (MIT/Apache-2.0). Our probe found every AirPlay device on this network from
  Rust, on macOS, next to mDNSResponder.
- **License:** Uzumaki is **GPL-3.0-or-later** (`Cargo.toml`, `LICENSE`), not closed. So GPL helpers are fine.
  A separate process is also only "aggregation" under the GPL.

## 1. Devices on this network (read-only mDNS probe)

Probe: `dns-sd -B/-L` and a small Rust program with `mdns-sd` 0.13 (in `/tmp`, browse only, 5 s). It sent no audio
and opened no connection to any device. Bit numbers are from the openairplay feature table (source 20).

| Name | Model (`am`/`model`) | What it is | `_raop._tcp` | AirPlay 2 only? | Notes |
|---|---|---|---|---|---|
| Bedroom | AudioAccessory5,1 | HomePod mini | yes | no | bits 27 (legacy pairing), 30 (RAOP), 40 (buffered), 41 (PTP), 48 (transient) set. No `pw`. **Best first target.** |
| Living Room | AudioAccessory6,1 | HomePod (2nd gen) | yes | no | One half of a **stereo pair** (same `tsid`, `tsm=2`). The pair is also the Apple TV's home theater audio (`gpn=Home Theater`). |
| Living Room (3) | AudioAccessory6,1 | HomePod (2nd gen) | yes | no | Other half of the same pair. |
| Living Room (2) | AppleTV14,1 | Apple TV 4K (3rd gen) | yes | no | Leader of the home-theater group (`psgsz=3`). Apple TVs may ask for HomeKit pairing with a PIN. |
| Smart TV Pro | TCL 55C745 | TCL TV | **no** | **yes** | Only `_airplay._tcp`. Bits 27 and 30 are **not** set; 38, 40, 41, 48 are. AirPlay 2 only. |
| (this Mac) | Mac17,8 | the Mac itself as a receiver | yes | no | Hide our own host in the picker. |

All devices advertise `acl=0` and no `pw=true`, so no password is set.

What this means:

- An AirPlay 1-only sender (libraop, node `airtunes2` in RAOP mode, PipeWire's RAOP sink) would reach the HomePods
  and the Apple TV, but **not the TCL TV**. `cliairplay --protocol auto` picks AirPlay 2 for all of them.
- The stereo pair and the home-theater group are the hard cases. Apple's own picker shows the pair as one speaker.
  First test a single HomePod mini, then the pair.

## 2. How Cider does it

### Cider 1 (open source, `ciderapp/Cider`, last commit 2024-12)

Read in source:

- **Capture** (`src/renderer/audio/audio.js`): `new AudioContext({sampleRate: 96000, latencyHint: "playback"})`,
  then `createMediaElementSource(mediaElem)`, then a gain chain. `sendAudio()` loads an inline `AudioWorklet`
  (`recorder-worklet`). It collects 2048 frames of Float32 per channel and posts them to the page. The page sends
  `ipcRenderer.send("writeWAV", left, right, 2048)`, which is about 21 ms of audio per message. It drops all-zero
  buffers.
- **Send to AirPlay** (`src/main/plugins/raop.ts`): it finds devices with `mdns-js` on `_raop._tcp` and
  `_airplay._tcp` and checks bit 9 (audio). It sends with **`airtunes2`** (fork `ciderapp/node_airtunes2`), which is
  RAOP (ALAC, 352-frame packets, 44.1 kHz, NTP) plus HomeKit or transient pairing for newer devices. The README
  says it was tested on Apple TV HD and HomePod mini.
- **Format conversion** (in a worker thread): Float to Int16, then 96 kHz to 44.1 kHz by picking samples, with no
  low-pass filter (aliasing). We should not copy this.
- **Local mute:** when the device is ready, Cider calls `webContents.setAudioMuted(true)`.
- **Chromecast** (`src/main/plugins/chromecast.ts`, `castv2-client`): Cider runs a local HTTP server that serves
  `http://<lan-ip>:<port>/audio.wav` (endless WAV, Int16 with dither). A custom cast receiver app plays that URL.
  The same `writeWAV` stream feeds it. It also has UPnP/DLNA.

### Cider 2 / 4 (closed source)

- The changelog only says "early AirPlay support" (Nov 2022). We found no AirPlay 2 or Chromecast docs.
- Open issues: the AirPlay or Cast menu is missing (#2241 on macOS 27, #2261, #2164). Linux AirPlay errors (#588,
  #642). Chromecast connects but sound stays on the Mac (#557). LG TV shows the wrong duration (#597). Many issues
  closed as stale with no answer. A forum report says Apple TV connects with no sound on Linux.
- A debug dump in #2241 shows a new `audioConversion.capture` block (`sinkOwner: "chromium"`). This is only a hint
  of a new capture path.

What we learn: the idea works in production, but Cider's sending part is fragile. Their sender is a Node
RAOP library with AirPlay 2 pairing added later. We should use a maintained sender.

## 3. Does Web Audio capture work on Widevine audio?

**Yes, by the Chromium source, and by Cider in practice.**

- `media_element_audio_source_handler.cc`: `Process()` outputs zeros only when `is_origin_tainted_`
  ("MediaElementAudioSource outputs zeroes due to CORS access restrictions").
- `WebMediaPlayerImpl::WouldTaintOrigin()` is set only by `OnDataSourceTainted()` (cross-origin data without CORS).
  MusicKit plays MSE from a blob URL of the same page, so this does not apply.
- `web_audio_source_provider_impl.cc` and the handler have no code about encryption or the CDM. The CDM decrypts
  before the audio renderer, and Web Audio taps the decoded audio after that.
- On desktop, Widevine audio is decoded in software. A "hardware secure" path that skips the renderer would apply
  to video (L1), not to Apple Music's AAC audio. **The spike must still check this on macOS** (castLabs VMP build)
  and on Linux.
- Side effects of `createMediaElementSource`: from that moment the element's sound goes **only** through the graph.
  We cannot undo it while the element lives. So we create it the first time the user starts casting, and keep the
  graph (source → local gain → speakers, plus source → worklet).

## 4. What other apps do

| App | In-app AirPlay? | How |
|---|---|---|
| Spotify desktop | No own AirPlay sender found | Spotify Connect for its own devices; AirPlay goes through the OS. Community threads, no official page. |
| Roon | Yes | Own sender; AirPlay 2 since 2.0.37; falls back to AirPlay 1 when a HomePod refuses the AirPlay 2 handshake. |
| Airfoil (Rogue Amoeba) | Yes, Mac and Windows | Captures app audio, own AirPlay 2 sender. Linux: receiver only. |
| OwnTone (server) | Yes | `src/outputs/raop.c` (AirPlay 1) and `airplay.c` (AirPlay 2: transient pairing, pair-verify, PTP). GPL-2.0. Best code reference. |
| Music Assistant | Yes | Python server runs **`cliairplay`** per speaker, PCM on stdin. Replaced their old `cliraop`/`cliap2`. |
| pyatv | RAOP only for audio | Verified on HomePod mini, Apple TV 4K gen 1, AirPort Express. MIT. |
| AirConnect | No (other direction) | Makes Chromecast/UPnP speakers look like AirPlay receivers. |
| shairport-sync | Receiver | Useful for protocol docs and as a fake test speaker on Linux. |
| Tidal, Plexamp | Not researched | No evidence collected either way. |

## 5. Sender options

| Option | Lang | License | AirPlay 2 | Maturity | Verdict |
|---|---|---|---|---|---|
| **`music-assistant/airplay-cli` (`cliairplay`)** | C/C++ | GPL-3.0 (bundles OpenSSL static, 3.5.4 at v0.5.10 on macOS arm64, not 1.1.1 as first noted; some libraop files have no explicit license) | Yes: native (transient / pair-verify, PTP or NTP, realtime or buffered), RAOP-compat, RAOP | Active (2026-11-13), used by every Music Assistant user, HomePod mini OS 27.2 verified in its DESIGN.md | **Chosen.** Helper process. |
| OwnTone `outputs/airplay.c` | C | GPL-2.0 | Yes | Mature, but part of a server (event loop, its own player) | Reference only. Hard to take out. |
| `philippe44/libraop` | C | MIT | No (AirPlay 1) | Mature | Would miss the TCL TV and future AirPlay 2-only devices. |
| `cidermill-airplay` crate | Rust | GPL-3.0-or-later | Yes (claims) | 0.1.5, two weeks old, 213 downloads | Watch it. Could replace the helper later. |
| `audiosync` app | Rust | GPL-3.0-or-later | Yes (claims, up to 16 speakers) | Beta, 8 stars | Read as a reference. |
| `airplay2-rs` (lmcgartland) | Rust | GPL-3.0 | Yes | Young | Watch. |
| node `airtunes2` (Cider) | JS | MIT | Partial | Cider only | No: would put the sender in the bridge. |
| PipeWire `module-raop-sink` | C | MIT | No (draft MR 1658) | System module | Linux fallback only: it is a sink, so `setSinkId` could send only Uzumaki there. AirPlay 1 limits; it needs the user to load `raop-discover`. |
| `alac-encoder` crate | Rust | MIT/Apache-2.0 | n/a | Stable, old | Not needed with `cliairplay` (it encodes ALAC itself). |

Why a helper and not a library: the AirPlay 2 handshake (SRP, HomeKit TLV8, ChaCha20, PTP) is large and changes
with each HomePod OS. Music Assistant fixes it for many users. A helper process also keeps a C crash out of the app.
We pin a tag, build it in CI for Linux x86_64/arm64 and macOS arm64/x86_64, and ship it next to the bridge.

Licensing note: libraop's prebuilt OpenSSL at v0.5.10 is already 3.x (Apache-2.0, GPL-3 compatible), so there is no
GPL/OpenSSL 1.1.1 clash. The macOS build links Homebrew's openssl@3 statically (3.5.9), falling back to the prebuilt one.

## 6. Recommended architecture

```
 bridge (castLabs Electron, renderer)                 Rust app                          helper
 ┌──────────────────────────────────────┐            ┌─────────────────────────┐       ┌────────────────────┐
 │ <audio> ─ MediaElementSource ─┬─ localGain ─ out   │ cast::Session           │       │ cliairplay          │
 │                               └─ AudioWorklet      │  - picks device (mDNS)  │ stdin │  (one per speaker)  │
 │   (ctx 44.1 kHz, f32→s16le,     │ 4096-frame chunks │  - PCM in ─────────────────────▶ ALAC, RTP, pairing │
 │    interleaved)                 ▼                   │  - cmdpipe: START/FLUSH │──────▶  START_UNIX_MS       │
 │ preload: MessagePort → ipc → main.js ──Unix socket─▶│  - latency → UI clock   │       └────────────────────┘
 └──────────────────────────────────────┘            │ local files: rodio tap ─┘
                                                      └─────────────────────────┘
```

1. **Discovery (Rust, `mdns-sd`):** browse `_airplay._tcp` and `_raop._tcp`, merge by `deviceid`, keep devices with
   bit 9 (audio), hide this host. Keep the TXT records: `cliairplay --protocol auto` needs them. Show stereo pairs
   and groups as one row (`tsid` / `gid`).
2. **Picker (Rune):** an output button in the player bar, a popover: "This computer" plus AirPlay speakers. Same UI
   on Linux and macOS. On macOS we can keep the old CoreAudio list from `airplay-macos.md` as extra rows.
3. **Capture (bridge):** when casting starts, the preload builds the graph once:
   `AudioContext({ sampleRate: 44100, latencyHint: 'playback' })`. Chromium then resamples properly, unlike Cider's
   sample picking. Graph: source → `localGain` → destination, and source → worklet → zero gain → destination (keeps
   the worklet running). The worklet converts to **s16le interleaved**, 4096-frame chunks (~93 ms, 16 KB). It
   posts them with transferable buffers.
4. **Transport (bridge → Rust):** not the JSON stdin/stdout. Rust creates a Unix socket (macOS + Linux), and passes
   its path in an env var as it does for other switches. `main.js` connects and writes raw PCM frames. The preload
   gets them to `main.js` with `ipcRenderer.send` (176 KB/s is small) or a `MessagePort`. Windows later: a named
   pipe. Shared memory is not worth it at this rate.
5. **Send (Rust):** `cast::Session` spawns `cliairplay --protocol auto --txt … --cmdpipe … <ip>` and pumps the
   socket into its stdin. It reads `[STATUS]` lines (connected, `buffered_ms`, `started`, `auth_required`) and turns
   them into UI states. Volume uses the helper's volume command (the speaker's own volume). MusicKit's volume stays
   as it is.
6. **Local mute:** `localGain.gain = 0` while casting. We do **not** touch `mk.volume`, which MusicKit saves.
   `webContents.setAudioMuted` (Cider's way) also works. But it mutes the whole page, and `UZUMAKI_MUTE` uses the
   same Chromium mute, so the gain node is cleaner. The spike checks whether `element.volume` scales the tapped
   audio (Chromium's `WebAudioSourceProviderImpl` applies the element volume). If yes, set `mk.volume` to 1 at
   cast start and drive the speaker volume instead, without saving it.
7. **Local files:** while casting, `local/player.rs` does not open a cpal stream. A small "PCM sink" thread pulls
   the rodio `Source` (resampled to 44.1 kHz stereo, `UniformSourceIterator`), converts to s16le, and writes to the
   same `cast::Session` input. The helper's pipe sets the pace: it reads at real time. So pause or seek is a FLUSH
   plus a new START.
8. **Pause, seek, next:** the bridge already sends state events. On pause or seek: cmdpipe `ACTION=FLUSH`, then
   `START_UNIX_MS` when audio flows again. While paused, the worklet sends nothing (Cider also drops silent chunks).
9. **Clock, progress bar, lyrics:** the speaker plays L seconds after the capture (about 2 s on RAOP, less on
   AirPlay 2; `cliairplay` reports the real start time). The UI shows `currentTime − L` while casting, and the
   lyrics clock uses the same value. As Thomas asked, lyrics tuning comes **after** a live test. Stage 3 only adds
   the offset hook with a constant.

Chromecast later: the same PCM socket can feed a small HTTP server in Rust (`/audio.wav` or FLAC). The default
media receiver plays the URL (Cider's way, but with Google's default receiver, so we need no receiver app).

## 7. Staged plan

| Stage | Work | Proves | Size |
|---|---|---|---|
| 0 | This research. Probe the devices (done). | Device list, AirPlay 2-only TV found | done |
| 1 | **Spike A, capture:** in a scratch profile (`UZUMAKI_PROFILE_DIR=/tmp/…`, `UZUMAKI_MUTE=1`, hidden), play an Apple Music **preview** (works signed out). Worklet → write the RMS of each chunk to the bridge log, **not the audio**. Check non-zero RMS on macOS and on Linux. | Web Audio sees Widevine PCM (needs a signed-in profile for full songs: Thomas runs that check) | 0.5 day |
| 2 | **Spike B, send:** build `cliairplay` locally. A throwaway Rust CLI: mDNS → pick "Bedroom" (HomePod mini) → spawn the helper → feed PCM from the bridge socket. **Thomas runs it live** (agents never send audio). Then try the stereo pair and the TCL TV. | Capture → send works end to end, latency number | 1–1.5 days |
| 3 | App: `cast` module (discovery, session, helper lifecycle, status), bridge socket and graph, local mute, Rune picker, time offset. | Usable feature, Apple Music path | 2–3 days |
| 4 | Local files into the same session (rodio tap, FLUSH/START on seek). | Both paths | 1 day |
| 5 | Packaging: build `cliairplay` in CI (Linux AppImage/AUR/deb, macOS bundle, signed), license notices, OpenSSL 3 build. | Ships | 1–1.5 days |
| 6 | After live tests: lyrics and progress fine-tuning, stereo pair / group polish, multi-speaker (shared PTP daemon). | Polish | 1–2 days |
| later | Chromecast via HTTP stream. | — | 1.5 days |

## 8. Risks

- **Apple TV and HomeKit:** Apple TVs may ask for a PIN pairing (stored `--auth` credentials). We need a "type the
  PIN shown on the TV" dialog. HomePods take transient pairing when Home → "Allow Speakers & TV Access" is set to
  everyone or the same network. If not, we get `auth_required`, and we must say so in the UI.
- **Stereo pair and home theater:** the two HomePods here are a pair and are tied to the Apple TV. Sending to one
  member may play on one side only, or be refused. Test this in stage 2.
- **AirPlay 2-only TV:** needs the AirPlay 2 native path. TVs that advertise PTP but never answer clock probes are
  handled by `cliairplay` (it forces NTP). Still, test it.
- **Helper upkeep:** `cliairplay` is young as a unified binary (it replaced cliraop/cliap2 in 2026). Pin a tag and
  update it on purpose. HomePod OS updates sometimes break third-party senders (Roon, OwnTone, Music Assistant all
  had such breaks).
- **Licensing:** GPL-3.0 helper is fine for us. The unlicensed libraop files need a
  look before we ship (OpenSSL is 3.x, Apache-2.0).
- **Firewall:** AirPlay needs inbound UDP for timing/control (NTP, and PTP on 319/320). macOS asks once. On Linux
  `ufw`/`firewalld` may block it, so document the ports. If PTP ports are taken, `cliairplay` falls back to NTP.
- **macOS capture under VMP:** expected fine (software audio decode), but proven only by the stage 1 spike.
- **Latency:** ~2 s on RAOP. The progress bar and lyrics must use the offset. Seeks feel slow (FLUSH + rebuffer,
  250 ms minimum on AirPlay 2).
- **Two outputs at once:** "This computer + speaker" in sync is hard (local output has ~0 delay). The first version
  is one output at a time.
- **Rule check:** we only tap the player's output, in memory, and stream it to the user's own speaker. We save no
  file, keep no long buffer, and do not touch keys or the CDM.

## Sources

1. Cider 1 repo: https://github.com/ciderapp/Cider
2. Cider capture: https://github.com/ciderapp/Cider/blob/main/src/renderer/audio/audio.js
3. Cider AirPlay: https://github.com/ciderapp/Cider/blob/main/src/main/plugins/raop.ts
4. Cider Chromecast: https://github.com/ciderapp/Cider/blob/main/src/main/plugins/chromecast.ts
5. airtunes2 fork: https://github.com/ciderapp/node_airtunes2
6. Cider HomePod PR: https://github.com/ciderapp/Cider/pull/917
7. Cider 2 changelog: https://v2.cider.sh/changelogs/client-releases/x-gc
8. Cider 2 issues: https://github.com/ciderapp/Cider-2/issues (#2241, #2261, #2164, #557, #588, #642, #597)
9. Chromium `media_element_audio_source_handler.cc`: https://github.com/chromium/chromium/blob/main/third_party/blink/renderer/modules/webaudio/media_element_audio_source_handler.cc
10. Chromium `web_media_player_impl.cc`: https://github.com/chromium/chromium/blob/main/third_party/blink/renderer/platform/media/web_media_player_impl.cc
11. Chromium `web_audio_source_provider_impl.cc`: https://github.com/chromium/chromium/blob/main/third_party/blink/renderer/platform/media/web_audio_source_provider_impl.cc
12. cliairplay: https://github.com/music-assistant/airplay-cli (README, DESIGN.md, THIRD_PARTY_NOTICES.md)
13. Music Assistant server: https://github.com/music-assistant/server
14. OwnTone: https://github.com/owntone/owntone-server (`src/outputs/airplay.c`, `src/outputs/raop.c`)
15. libraop: https://github.com/philippe44/libraop
16. AirConnect: https://github.com/philippe44/AirConnect
17. pyatv supported features: https://pyatv.dev/documentation/supported_features/
18. PipeWire RAOP sink: https://docs.pipewire.org/devel/page_module_raop_sink.html, AirPlay 2 MR: https://gitlab.freedesktop.org/pipewire/pipewire/-/merge_requests/1658
19. shairport-sync: https://github.com/mikebrady/shairport-sync
20. AirPlay feature bits: https://openairplay.github.io/airplay-spec/features.html
21. mdns-sd: https://github.com/keepsimple1/mdns-sd
22. alac-encoder: https://github.com/LinusU/rust-alac-encoder
23. cidermill-airplay: https://gitlab.com/timrs2998/cidermill-airplay
24. audiosync: https://github.com/yassersutojoyo/audiosync
25. airplay2-rs: https://github.com/lmcgartland/airplay2-rs
26. Roon AirPlay 2 / HomePod fallback: Roon community release notes 2.0.37
27. Airfoil and AirPlay 2: https://weblog.rogueamoeba.com/2018/06/25/current-notes-on-airfoil-airplay-2/
28. Spotify community, AirPlay on Mac: https://community.spotify.com/t5/Desktop-Mac/Airplay-devices-not-showing-up-in-mac-app/td-p/5496807

## Spike results (2026-11-14, branch `airplay-spike`)

Stages 1-2 are built behind `UZUMAKI_AIRPLAY_SPIKE=1`; how to run them: `scripts/airplay-spike/README.md`.

- **Capture:** the AudioWorklet graph runs in castLabs Electron on macOS (the page allows a blob:
  worklet module). Signed-out previews give zeros because they are cross-origin files without
  `crossOrigin` (CORS taint, as section 3 predicts). With the test switch `UZUMAKI_AIRPLAY_PREVIEW_CORS=1`
  they measure about -16 dBFS while the local output is muted. Full songs (same-origin `blob:`, Widevine)
  need a signed-in run: that is the first thing the live test shows.
- **Element volume:** MusicKit's volume scales the tapped audio (section 6's question), so casting
  loudness = app volume × speaker volume.
- **Transport and sender:** Unix socket → Rust (`airplay/` crate, `mdns-sd` discovery) → `cliairplay`
  v0.5.10 (built from source on macOS arm64). Checked end to end with a stand-in helper and with the real
  helper against a closed port (error lines, retries). No audio was sent to any AirPlay device by agents.
- **Local files (stage 4, early, same switch):** a rodio `Source` wrapper (`app/src/local/tap.rs`) copies
  the decoded samples (linear resample to 44.1 kHz stereo s16, × app volume) to the same helper session and
  sends zeros to the Mac's output while casting (volume setting untouched). One session serves both
  players: the bridge's bytes while an Apple song plays, the file's while one is loaded, silence while it
  is paused. Not done yet from section 6 item 7: no FLUSH/START on seek (the seek is heard after the lead),
  and the sound card stays open (muted) to pace the file. Checked headless with the stand-in helper only.
