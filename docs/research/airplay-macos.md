# AirPlay inside Uzumaki on macOS: research

Date: 2026-11-14. Research only. This note changes no code.

> **Update 2026-11-14:** the rule changed. Uzumaki may now send the player's already-decrypted output to the
> user's own speakers. See `airplay-sender.md` for the in-app AirPlay sender plan. The Cider note below
> ("Not for us") is out of date.

## The question

We want an AirPlay button inside Uzumaki. Can we send **only Uzumaki's audio** to an AirPlay speaker?
Or can we only change the output for the whole Mac?

## Verdict

| Path | Send only Uzumaki to a device we pick? | Send only Uzumaki to an AirPlay speaker? |
|---|---|---|
| Apple Music (castLabs Electron, `<audio>` + Widevine) | **Yes**, for any CoreAudio device (`setSinkId`). | **No, not by itself.** It works only when the user first turns AirPlay on for the Mac. |
| Local files (rodio/cpal) | **Yes**, cpal opens any CoreAudio device by name. | **Same limit as above.** |

Short reason:

- Each app can pick its own CoreAudio output device. Chromium and cpal both do this. This is per-app.
- But the modern "AirPlay" CoreAudio device is not a list of speakers. There is **one** device called "AirPlay".
  It has no data sources, so an app cannot choose a speaker with it. It points to whatever speaker the user picked
  in the system picker (Control Center or `AVRoutePickerView` with no player). Picking there also makes it the
  **system** output.
- Per-app AirPlay with a free choice of speaker exists only for `AVPlayer` (via `AVRoutePickerView.player`).
  Our Apple Music audio is Widevine in Chromium, not an `AVPlayer`, so that path is closed to us. Even for
  `AVPlayer`, developers report that it is buggy on macOS (sources 3, 4).

So: **in practice we are limited to the system-wide output for choosing an AirPlay speaker.** After that, we can
choose per-app whether Uzumaki uses it, but that gives little value.

## What we checked

### 1. `AVRoutePickerView` on macOS

- The SDK header (`AVKit/AVRoutePickerView.h`) has only one routing knob on macOS: `player` (`AVPlayer`, macOS 10.15+).
  There is no way to give it a CoreAudio device, an audio unit, or a process.
- `player` set: the picker routes that `AVPlayer` only. It does not route other audio in the process.
- `player` nil: the picker shows the same list as Control Center and changes the **system** output.
- Forum reports: with `player` set, a HomePod pick can leave the player on the Mac speakers (macOS 12).
  `ApplicationMusicPlayer` (MusicKit native) ignores the picker on macOS 14 (FB13521393). No Apple answer.
- It does not move Chromium's audio, except as a side effect of changing the system output.

### 2. Per-app routing model on macOS (there is no `AVAudioSession`)

- On macOS each output is a CoreAudio device. Each app picks a device by UID:
  - `AVPlayer.audioOutputDeviceUniqueID` (macOS 10.9+),
  - `AVSampleBufferAudioRenderer.audioOutputDeviceUniqueID` (macOS 10.13+),
  - AUHAL `kAudioOutputUnitProperty_CurrentDevice`.
  The SDK header says the UID comes from `kAudioDevicePropertyDeviceUID`. This does **not** change the system default.
- AirPlay devices: transport type `kAudioDeviceTransportTypeAirPlay` (`'airp'`).
- Our probe (a read-only Swift CLI, on macOS 27.2, with AirPlay already on from Control Center) found:
  - one device named `AirPlay`, transport `airp`, 48 kHz, stream latency 14160 frames (about 0.30 s);
  - `kAudioDevicePropertyDataSources` and `kAudioDevicePropertyDataSource`: **not present** on that device;
  - the built-in speakers, as a second device.
- Old model (about 2013, OS X 10.8): the "AirPlay" device had one data source per speaker, so an app could pick one.
  Reports say this broke with El Capitan. On Sierra the device appeared only after the user picked AirPlay in
  System Preferences (source 7). Our probe matches: no data sources today.
- AirPlay 2 multi-room (several speakers at once) is a system or `AVPlayer` route feature. CoreAudio only sees the
  one "AirPlay" device.
- Open point, not tested (it would change the system output or play audio): if the user picks AirPlay and then
  sets the Mac back to its speakers, does the "AirPlay" device stay, and can Uzumaki keep using it? If yes, a
  per-app trick is possible, but it is fragile and undocumented. VLC has a code comment that a gone AirPlay device
  can still report "alive" (source 9).

### 3. Electron / Chromium (the Apple Music path)

- MusicKit plays through `<audio id="apple-music-player">`. `bridge/preload.js` already finds it for the lyrics clock.
  So `mediaEl.setSinkId(id)` is possible from the preload.
- Chromium lists all CoreAudio output devices. It skips only private aggregate devices
  (`audio_manager_mac.cc`). It does **not** filter AirPlay. The label gets a transport suffix, so the device
  shows as `AirPlay (AirPlay)` (`core_audio_util_mac.cc`, `TransportTypeToString`).
- `enumerateDevices()` gives per-origin hashed IDs. Labels need permission. castLabs Electron 45 has the
  `speaker-selection` and `media` permission types in `setPermissionCheckHandler`/`setPermissionRequestHandler`
  (checked in `electron.d.ts`). The bridge sets no handler now. We should grant only those two, only for
  music.apple.com.
- `selectAudioOutput()` shows a browser prompt. We do not need it: the Rust UI shows the list.
- `setSinkId` with EME/Widevine: Widevine on desktop decrypts and decodes in the renderer. The audio then goes
  through the normal audio sink, so `setSinkId` should work. We found no doc that says so for castLabs. **Test it
  first** (stage 1).
- Our bridge disables `AudioServiceOutOfProcess`, so the audio stream lives in the bridge process. This does not
  change device choice.

### 4. Local files (rodio/cpal)

- Our cpal 0.16 probe (no audio played) listed `AirPlay` and `MacBook Pro Speakers`, each with 48 kHz stereo.
- rodio 0.21 has `OutputStreamBuilder::from_device(device)`. Today `app/src/local/player.rs` uses
  `open_default_stream()`. A change is small: find the device by name and open it.
- cpal names, not UIDs, so two devices with the same name are a small risk.

### 5. What other apps do

- **IINA / mpv:** an "Audio device" menu that lists CoreAudio devices (`--audio-device=coreaudio/<UID>`).
  AirPlay shows only as the one "AirPlay" device after the system picks it.
- **VLC:** "Audio Device" menu over AUHAL. Same model. It has bugs where AirPlay stays selected after the speaker
  is gone (source 8).
- **Music.app, QuickTime, Safari video:** real per-app AirPlay. They use `AVPlayer` / private AirPlay routing.
  We cannot use these for Widevine audio.
- **Cider (Electron Apple Music client):** lists AirPlay as a feature. The old open-source Cider streamed PCM to
  AirPlay speakers with its own RAOP sender. **Not for us:** it sends the decrypted audio out of the Widevine
  player to another device. That is against our DRM rule. Also modern AirPlay 2 speakers often need HomeKit pairing.
- **Spotify:** its device menu is Spotify Connect. We found no proof of in-app AirPlay on macOS.

### 6. Latency and synced lyrics

- AirPlay buffers. The probe reads 0.30 s of stream latency on the AirPlay device. The real delay can be larger
  (AirPlay 2 buffers up to about 2 s).
- Chromium adds unit, device and stream latency (`GetHardwareLatency`) to the audio delay. The `<audio>`
  `currentTime` already counts this delay. So our lyrics clock follows what Chromium reports.
- Live test (2026-11-11, AirPlay from Control Center): lyrics lagged a little but felt normal.
- If we need more: read `kAudioDevicePropertyLatency` + `kAudioStreamPropertyLatency` +
  `kAudioDevicePropertySafetyOffset` in Rust (as the probe does) for the local path. `AudioContext.outputLatency`
  applies only to Web Audio, not to `<audio>`. A user offset slider is the safe fallback.

### 7. UI: `AVRoutePickerView` or our own list

- `AVRoutePickerView` is an `NSView`. Rune draws to a GPU surface, so we cannot put it in the layout like a
  widget. We could add it as a subview of the window's content view (via `objc2`), or put it in a tiny
  borderless panel and send it a click to open its menu. Both are hacks: wrong look, focus issues, and the menu
  position follows the hidden view.
- Its only effect for us (no `player`) is to change the system output. That is the same as Control Center.
- A Rune list of CoreAudio devices fits our design better and works the same on Linux (PipeWire sinks).

## Recommended architecture

1. **Rust (`app/`, macOS):** a small `audio_devices` module with `objc2-core-audio` (or the raw C API). It lists
   output devices (name, UID, transport type, latency) and listens for device-list changes.
2. **Rune UI:** an output button in the player bar. It opens a popover with "This Mac (System)" plus each device.
   AirPlay devices get the AirPlay icon. Add one row "AirPlay speakers…" that opens the system Sound menu
   (Control Center), with a short hint: pick a speaker there, then it shows up here.
3. **Apple Music path:** a new bridge message `setOutput { label | "default" }`. The preload calls
   `enumerateDevices()`, matches the label, and calls `setSinkId` on the MusicKit `<audio>` element. Apply it again
   when MusicKit replaces the element. Grant `speaker-selection`/`media` checks only for music.apple.com.
4. **Local path:** open the rodio stream with the chosen cpal device. Fall back to default if it is gone.
5. **Optional, later:** an `AVRoutePickerView` in a small native panel, only as a shortcut to the system picker.

## Effort (stages)

| Stage | Work | Size |
|---|---|---|
| 1 | Spike: in a scratch profile, list devices in the bridge and call `setSinkId` on the protected `<audio>`. Muted. Proves EME + `setSinkId`. | 0.5 day |
| 2 | Rust device list + change events (macOS), bridge message, preload `setSinkId`, permissions | 1 day |
| 3 | Rune popover, persisted choice, fallback on unplug | 1 day |
| 4 | Local path: rodio `from_device` + fallback | 0.5 day |
| 5 | Linux parity (PipeWire sinks through the same UI) | 1 day |
| 6 | Optional: native route-picker shortcut | 0.5–1 day |

## Risks

- Per-app AirPlay speaker choice is **not** possible with public APIs for non-`AVPlayer` audio. The button can
  only send Uzumaki to the "AirPlay" device after the user turns AirPlay on for the whole Mac.
- `setSinkId` on Widevine audio in castLabs Electron is untested.
- The "AirPlay" device appears and disappears. It can look "alive" after it is gone (VLC note). Always check the
  device list, and fall back to default.
- `enumerateDevices` labels need a permission grant. IDs change per origin and per profile, so match by label.
- Latency on AirPlay is only what CoreAudio reports. The true delay may be larger.
- Private APIs (MediaRemote, AirPlay frameworks) could do more, but they break between macOS releases. Avoid them.

## Sources

1. Apple, `AVRoutePickerView`: https://developer.apple.com/documentation/avkit/avroutepickerview
2. Apple, `AVRoutePickerView.player`: https://developer.apple.com/documentation/avkit/avroutepickerview/player
3. Apple forums, "AVRoutePickerView player property": https://developer.apple.com/forums/thread/708248
4. Apple forums, "ApplicationMusicPlayer on macOS doesn't work with AirPlay": https://developer.apple.com/forums/thread/744128
5. Apple forums, "macOS program audio output to AirPlay": https://developer.apple.com/forums/thread/698988
6. Apple forums, DTS on AirPlay routes (iOS): https://developer.apple.com/forums/thread/810035
7. JUCE forum, "AirPlay streaming" (old data-source model): https://forum.juce.com/t/airplay-streaming/11260
8. VLC issue, AirPlay retained as output: https://code.videolan.org/videolan/vlc/-/issues/17863
9. VLC AUHAL source: https://code.videolan.org/videolan/vlc/-/blob/master/modules/audio_output/apple/auhal.c
10. Chromium `audio_manager_mac.cc`: https://chromium.googlesource.com/chromium/src/+/main/media/audio/mac/audio_manager_mac.cc
11. Chromium `core_audio_util_mac.cc`: https://chromium.googlesource.com/chromium/src/+/main/media/audio/mac/core_audio_util_mac.cc
12. W3C Audio Output Devices API: https://www.w3.org/TR/audio-output/
13. MDN `HTMLMediaElement.setSinkId`: https://developer.mozilla.org/docs/Web/API/HTMLMediaElement/setSinkId
14. MDN `speaker-selection` policy: https://developer.mozilla.org/docs/Web/HTTP/Headers/Feature-Policy/speaker-selection
15. Chrome blog, `AudioContext.setSinkId`: https://developer.chrome.com/blog/audiocontext-setsinkid
16. Cider feature table: https://cider.sh/learn-more
17. SDK headers (local): `AVKit/AVRoutePickerView.h`, `AVFoundation/AVPlayer.h`,
    `AVFoundation/AVSampleBufferAudioRenderer.h`, `CoreAudio/AudioHardwareBase.h`.
