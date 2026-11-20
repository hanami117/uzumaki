# Chromecast spike

Plays Uzumaki's output on a Cast device (Chromecast, Google TV, Nest speaker, Cast Group) through
Google's Default Media Receiver. Off unless Settings → Speakers → Chromecast is on (or the app is
started with `UZUMAKI_CAST_SPIKE=1`, a dev override); then Cast devices are in the AirPlay picker, and
`uzu cast` drives it too. Code: `cast/` (crate `uzumaki-cast`).

How it works: the app speaks CASTV2 to the device (TLS, port 8009), launches the Default Media Receiver
and `LOAD`s a URL served by Uzumaki itself: `http://<this computer>:<port>/cast/<random token>/stream.wav`
(or `.flac`). The stream is the player's own decoded output (the same capture as AirPlay), live, in
memory only. Only the device's address may fetch it (for a Cast Group: the Cast devices seen on the
network), the token changes with every `LOAD`, and the server ends with the session.

## 1. The test tool first (no app)

```sh
cargo build -p uzumaki-cast --release
./target/release/uzumaki-cast list
./target/release/uzumaki-cast play "<device name>" --volume 20          # a beep every second
./target/release/uzumaki-cast play "<device name>" --flac --volume 20   # the same as FLAC
./target/release/uzumaki-cast play "<device name>" --file some.wav      # 16-bit 44.1 kHz WAV
```

While it plays, type a letter and Enter: `p` pause and `r` resume (instant: the TV pauses at once and keeps its buffer, the stream waits), `l` reload (a new `LOAD`, what a seek
would do), `v 30` device volume, `s` status, `q` stop. The tool prints `beep` when it sends one: the
time until you hear it is the latency. The log lines `cast: buffer N ms` are the same number measured
from the device's own position reports.

`--volume N` sets the device's volume (0-100) once connected; without it the device keeps its own.
`--buffered` tells the receiver `streamType: BUFFERED` instead of `LIVE`. `--port P` fixes the HTTP port
(for a firewall rule).

## 2. In the app

```sh
UZUMAKI_CAST_SPIKE=1 <the uzumaki binary>        # optional: UZUMAKI_CAST_FORMAT=flac
uzu cast list
uzu cast "<device name>"
uzu cast status          # state, who fetched, LOAD→fetch→PLAYING times, the buffer
uzu cast volume 25       # the device's volume
uzu cast pause | resume  # the receiver's own pause (the stream keeps going)
uzu cast reload          # a new LOAD
uzu cast off             # back to this computer
```

Pausing in the app sends silence (the device keeps playing it). A seek plays out the device's buffer
first; with `UZUMAKI_CAST_RELOAD_ON_SEEK=1` a seek does a new `LOAD` instead. Picking an AirPlay
speaker ends the Cast session and the other way round.

## Reading the log

`cast: audio heard … ms after LOAD` is when the TV's position first moved (a `PLAYING` at 0 is still
waiting). `cast: buffer N ms` is the delay, measured from then on. `UNDERRUN` (buffering again after it
played) and `STALL` (`PLAYING` but the position stands still) mark trouble; `s` / `uzu cast status`
counts them, with the setup (format, framing, pre-roll, streamType, IPv4/IPv6).

## Firewall

The device fetches from this computer, so an incoming TCP connection must be allowed: macOS asks once
when the app firewall is on; on Linux open the port (`UZUMAKI_CAST_PORT=8090`, then e.g.
`sudo ufw allow from <device ip> to any port 8090 proto tcp`). A device that never fetches ends the
session after 15 s with "A firewall may be blocking it".

## Switches

| Variable | |
|---|---|
| `UZUMAKI_CAST_SPIKE=1` | Chromecast on whatever the setting says (dev override) |
| `UZUMAKI_CAST_FORMAT=wav\|flac` | stream format (default wav) |
| `UZUMAKI_CAST_PORT` | fixed HTTP port (default: any free one) |
| `UZUMAKI_CAST_STREAM_TYPE=LIVE\|BUFFERED` | what `LOAD` says (default LIVE) |
| `UZUMAKI_CAST_RELOAD_ON_SEEK=1` | a seek does a new `LOAD` |
| `UZUMAKI_CAST_IP=4\|6` (tool: `--ipv4`/`--ipv6`) | talk to the device over that family only; default: IPv4 when it has one on this computer's subnet, else IPv6 (then its whole /64 may fetch, since TVs fetch from temporary privacy addresses) |
| `UZUMAKI_CAST_RATE` / `UZUMAKI_CAST_BITS` (tool: `--rate 44.1k\|48k\|88.2k\|96k`, `--bits 16\|24`) | WAV format, default 96k/24. The Android TV receiver waits for a fixed number of **bytes** before it plays, so more bytes per second = less delay (live test 4: FLAC took twice as long as WAV 44.1/16) |
| `UZUMAKI_CAST_BURST_BYTES` (tool: `--burst`) | the automatic pre-roll: this many bytes of the stream at once (default 1 400 000 ≈ 2.4 s at 96k/24), so it starts at once |
| `UZUMAKI_CAST_PREROLL_MS` (tool: `--preroll MS`) | a fixed pre-roll instead (the last N ms of audio, silence for what isn't there yet). It is also the steady delay; lyrics and the progress bar follow the measured one |
| `UZUMAKI_CAST_HTTP=length\|close\|chunked` (tool: `--http`) | body framing, default `length` (a 4 GB Content-Length; best in live test 4). The stream then ends after 2 h at 96k/24 and the session loads a new URL by itself |
| `UZUMAKI_CAST_LOCAL=1` | keep this computer's output on while casting (A/B checks) |
| `UZUMAKI_CAST_HOST` (+ `UZUMAKI_CAST_DEVICE_PORT`, `UZUMAKI_CAST_TARGET`) | a device at that address, no mDNS (tests) |
