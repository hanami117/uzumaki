#!/usr/bin/env python3
"""Stand-in for cliairplay, for a dry run of the AirPlay spike with no speaker.

It answers like the real helper ([STATUS] connected, audio buffered, started), takes the START
from the command pipe and, instead of sending anything, prints the level of the PCM it receives
once a second (RMS in dBFS). The audio is only measured in memory, never written anywhere.

    UZUMAKI_CLIAIRPLAY=scripts/airplay-spike/fake-cliairplay.py UZUMAKI_AIRPLAY_HOST=127.0.0.1 ...

`--report <file>` (tests only, through UZUMAKI_AIRPLAY_EXTRA_ARGS) also appends the level lines to
that file.
"""
import math, os, struct, sys, time

args = sys.argv[1:]
pipe = args[args.index("--cmdpipe") + 1] if "--cmdpipe" in args else None
report = args[args.index("--report") + 1] if "--report" in args else None
def log(s):
    print(s, file=sys.stderr, flush=True)
    if report and s.startswith("fake speaker"):
        with open(report, "a") as f:
            f.write(s + "\n")
print("[STATUS] route protocol=fake flow=dry-run timing=none buffered=0", flush=True)
log("[STATUS] connected")
fd = os.open(pipe, os.O_RDWR | os.O_NONBLOCK) if pipe else None  # as cliairplay opens it

stdin = sys.stdin.buffer
chunk = 44100 * 4  # one second of s16le stereo
started = False
cmd = b""
while True:
    data = stdin.read(chunk)
    if not data:
        log("[STATUS] eof")
        break
    if not started:
        log("[STATUS] audio buffered_ms=93")
        deadline = time.time() + 5
        while b"ACTION=START" not in cmd and time.time() < deadline:
            try:
                cmd += os.read(fd, 4096)
            except BlockingIOError:
                time.sleep(0.05)
        started = True
        log("[STATUS] started requested_unix_ms=0 at_unix_ms=%d" % int(time.time() * 1000 + 250))
    n = len(data) // 2
    samples = struct.unpack("<%dh" % n, data[: n * 2])
    rms = math.sqrt(sum(s * s for s in samples) / n) / 32768 if n else 0
    peak = max(abs(s) for s in samples) / 32768 if n else 0
    db = "%.1f" % (20 * math.log10(rms)) if rms > 0 else "-inf"
    log("fake speaker: %d bytes, rms %.5f (%s dBFS) peak %.4f" % (len(data), rms, db, peak))
