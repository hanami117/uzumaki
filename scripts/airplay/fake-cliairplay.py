#!/usr/bin/env python3
"""Stand-in for cliairplay, for tests and dry runs with no speaker and no network.

It answers like the real helper: [STATUS] connected, audio buffered, started, flushed, stopped;
it takes START, FLUSH, VOLUME and STOP from the command pipe; and instead of sending anything it
measures the PCM it receives (level once a second). The audio is only measured in memory.

    UZUMAKI_CLIAIRPLAY=scripts/airplay/fake-cliairplay.py UZUMAKI_AIRPLAY_HOST=127.0.0.1 ...

What it does is chosen by FAKE_AIRPLAY (comma-separated words), so tests can play the failures:
    auth_required   refuse before connecting (a password or a pairing is needed)
    need_password   refuse unless --password is "secret" (wrong one: auth_failed)
    need_pin        refuse unless --auth is given (pair first: --pair-setup, the PIN is 1234)
    connect_failed  the speaker is off
    hang            never connect (the app's timeout must end it)
    stalled         connect, then report a clock nobody answers
    die_after=N     end without a word N seconds after the start (connection lost)
FAKE_AIRPLAY_JOURNAL=<file>: every command, state and level line is appended there.
FAKE_AIRPLAY_RAW=<file>: the PCM received, raw (tests only, in a temp folder that the test removes).
"""
import math, os, struct, sys, threading, time

args = sys.argv[1:]
mode = [m for m in os.environ.get("FAKE_AIRPLAY", "").split(",") if m]
journal = os.environ.get("FAKE_AIRPLAY_JOURNAL")
raw_path = os.environ.get("FAKE_AIRPLAY_RAW")
lock = threading.Lock()


def note(s):
    if journal:
        with lock, open(journal, "a") as f:
            f.write(s + "\n")


def status(s, out=sys.stderr):
    print("[STATUS] " + s, file=out, flush=True)
    note("status " + s)


def arg(name):
    return args[args.index(name) + 1] if name in args else None


def fail(code, http, detail):
    status('error code=%s http=%d detail="%s"' % (code, http, detail))
    print("[ERROR] " + detail, file=sys.stderr, flush=True)
    sys.exit(1)


if "--check" in args:
    print("cliairplay fake check")
    sys.exit(0)

if "--pair-setup" in args:
    note("pair-setup " + " ".join(args))
    print("Enter the PIN shown on the device:", file=sys.stderr, flush=True)
    pin = sys.stdin.readline().strip()
    note("pin " + pin)
    if pin != "1234":
        print("Pairing failed.", file=sys.stderr, flush=True)
        sys.exit(1)
    print("CREDENTIALS: " + "ab" * 96, flush=True)
    sys.exit(0)

note("args " + " ".join(args))
status("route protocol=fake flow=dry-run timing=none buffered=0", sys.stdout)
if "hang" in mode:
    time.sleep(3600)
if "connect_failed" in mode:
    fail("connect_failed", 0, "cannot reach the device")
if "auth_required" in mode:
    fail("auth_required", 401, "the device wants a password")
if "need_password" in mode:
    pw = arg("--password")
    if pw is None:
        fail("auth_required", 401, "the device wants a password")
    if pw != "secret":
        fail("auth_failed", 401, "wrong password")
if "need_pin" in mode and arg("--auth") is None:
    fail("auth_required", 403, "pair-setup refused by device")

pipe = arg("--cmdpipe")
status("latency warm_lead_ms=600 render_lead_ms=250", sys.stdout)
status("connected")
fd = os.open(pipe, os.O_RDWR | os.O_NONBLOCK) if pipe else None  # as cliairplay opens it
if "stalled" in mode:
    status("clock_ready mode=ptp state=stalled streak_ms=0 exchanges=0 ready_in_ms=0 ready_at_unix_ms=0")

state = {"buffered": False, "started": False, "stop": False, "flush": 0}
for m in mode:
    if m.startswith("die_after="):
        threading.Timer(float(m.split("=")[1]), lambda: os._exit(0)).start()


def commands():
    buf = b""
    while not state["stop"]:
        try:
            buf += os.read(fd, 4096)
        except BlockingIOError:
            time.sleep(0.02)
            continue
        while b"\n" in buf:
            line, buf = buf.split(b"\n", 1)
            line = line.decode().strip()
            if not line:
                continue
            note("cmd " + line)
            if line == "ACTION=START":
                state["started"] = True
                status("started requested_unix_ms=0 at_unix_ms=%d" % int(time.time() * 1000 + 250))
            elif line == "ACTION=FLUSH":
                with lock:
                    state["flush"] += 1
                state["started"] = False
                state["buffered"] = False
                status("flushed head_unix_ms=%d" % int(time.time() * 1000))
            elif line == "ACTION=STOP":
                state["stop"] = True
                status("stopped")
                os._exit(0)


if fd is not None:
    threading.Thread(target=commands, daemon=True).start()

stdin = sys.stdin.buffer
second = 44100 * 4
got = b""
total = 0
while True:
    data = stdin.read1(16384)
    if not data:
        status("eof")
        break
    if not state["buffered"]:
        state["buffered"] = True
        status("audio buffered_ms=93")
    if raw_path:
        with open(raw_path, "ab") as f:
            f.write(data)
    got += data
    while len(got) >= second:
        chunk, got = got[:second], got[second:]
        n = len(chunk) // 2
        samples = struct.unpack("<%dh" % n, chunk)
        rms = math.sqrt(sum(s * s for s in samples) / n) / 32768
        peak = max(abs(s) for s in samples) / 32768
        db = "%.1f" % (20 * math.log10(rms)) if rms > 0 else "-inf"
        total += 1
        line = "fake speaker: second %d, rms %.5f (%s dBFS) peak %.4f" % (total, rms, db, peak)
        print(line, file=sys.stderr, flush=True)
        note(line)
