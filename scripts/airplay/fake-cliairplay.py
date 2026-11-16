#!/usr/bin/env python3
"""Stand-in for cliairplay, for tests and dry runs with no speaker and no network.

It answers like the real helper: [STATUS] connected, audio buffered, started, flushed, stopped;
it takes START (with START_UNIX_MS and START_JOIN), FLUSH, VOLUME and STOP from the command pipe;
and instead of sending anything it measures the PCM it receives (level once a second). The audio
is only measured in memory. `--ptp-daemon` (the shared clock) only waits to be ended.

    UZUMAKI_CLIAIRPLAY=scripts/airplay/fake-cliairplay.py UZUMAKI_AIRPLAY_HOST=127.0.0.1 ...

What it does is chosen by FAKE_AIRPLAY (comma-separated words), so tests can play the failures:
    auth_required   refuse before connecting (a password or a pairing is needed)
    need_password   refuse unless --password is "secret" (wrong one: auth_failed)
    need_pin        refuse unless --auth is given (pair first: --pair-setup, the PIN is 1234)
    connect_failed  the speaker is off
    busy            refuse every session at once (connect_failed), as a speaker playing for another device
    busy=N          refuse the first N sessions of this journal, then play (the speaker lets the other sender go)
    http=N          the HTTP status of a busy refusal (default 0: the connection itself was refused)
    hang            never connect (the app's timeout must end it)
    stalled         connect, then report a clock nobody answers
    die_after=N     end without a word N seconds after the start (connection lost)
    clock           report a clock that is ready shortly after connecting
A word written `<Name>:<word>` applies only to the speaker called <Name> (its --name), so one
journal can play a group where one speaker fails.
FAKE_AIRPLAY_JOURNAL=<file>: every command, state and level line is appended there.
FAKE_AIRPLAY_RAW=<file>: the PCM received, raw (tests only, in a temp folder that the test removes).
FAKE_AIRPLAY_DIR=<folder>: also <Name>.txt (that speaker's journal) and <Name>.pcm (its PCM) there.
"""
import math, os, struct, sys, threading, time

args = sys.argv[1:]
journal = os.environ.get("FAKE_AIRPLAY_JOURNAL")
raw_path = os.environ.get("FAKE_AIRPLAY_RAW")
per_dir = os.environ.get("FAKE_AIRPLAY_DIR")
lock = threading.Lock()


def arg(name):
    return args[args.index(name) + 1] if name in args else None


me = arg("--name") or (args[-1] if args else "")
mode = []
for m in os.environ.get("FAKE_AIRPLAY", "").split(","):
    if not m:
        continue
    who, sep, word = m.partition(":")
    if sep and not who.startswith("http") and not who.startswith("busy") and not who.startswith("die_after"):
        if who == me:
            mode.append(word)
    else:
        mode.append(m)
own_journal = os.path.join(per_dir, me + ".txt") if per_dir else None
own_raw = os.path.join(per_dir, me + ".pcm") if per_dir else None


def note(s):
    with lock:
        for path in (journal, own_journal):
            if path:
                with open(path, "a") as f:
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

if "--ptp-daemon" in args:
    note("ptp-daemon " + " ".join(args))
    try:
        time.sleep(3600)
    except KeyboardInterrupt:
        pass
    sys.exit(0)

note("args " + " ".join(args))
status("route protocol=fake flow=dry-run timing=none buffered=0", sys.stdout)
if "hang" in mode:
    time.sleep(3600)
def earlier_sessions():
    try:
        with open(own_journal or journal) as f:
            return sum(1 for l in f if l.startswith("args ")) - 1
    except (OSError, TypeError):
        return 0


busy_http = int(next((m.split("=")[1] for m in mode if m.startswith("http=")), "0"))
if "busy" in mode:
    fail("connect_failed", busy_http, "native AirPlay 2 connect failed")
for m in mode:
    if m.startswith("busy=") and earlier_sessions() < int(m.split("=")[1]):
        fail("connect_failed", busy_http, "native AirPlay 2 connect failed")
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
elif "clock" in mode:
    status("clock_ready mode=ptp state=cold streak_ms=0 exchanges=0 ready_in_ms=0 ready_at_unix_ms=0")
    threading.Timer(0.3, lambda: status("clock_ready mode=ptp state=ready streak_ms=0 exchanges=8 ready_in_ms=0 ready_at_unix_ms=0")).start()

state = {"buffered": False, "started": False, "stop": False, "flush": 0, "start_ms": 0, "join": False, "head": 0}
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
            if line.startswith("START_UNIX_MS="):
                state["start_ms"] = int(line.split("=", 1)[1])
            elif line.startswith("START_JOIN="):
                state["join"] = line.split("=", 1)[1] != "0"
            elif line == "ACTION=START":
                state["started"] = True
                # As the real helper: an instant it can't make (or 0) moves to the earliest it can,
                # 250 ms ahead and past the audio still queued from before a flush.
                earliest = max(int(time.time() * 1000 + 250), state["head"] + 250 if state["head"] else 0)
                req = state["start_ms"]
                at = req if req >= earliest else earliest
                status("started requested_unix_ms=%d at_unix_ms=%d" % (req, at))
                if state["join"] and req and at > req:
                    # A join that starts later than asked drops as much of its audio, so it stays
                    # in step with the group (the PCM file keeps everything received).
                    frames = ((at - req) * 44100 + 500) // 1000
                    status("content_cut requested_ms=%d cut_ms=%d cut_bytes=%d drain_ms=0" % (at - req, at - req, frames * 4))
                state["head"] = at
                state["start_ms"], state["join"] = 0, False
            elif line == "ACTION=FLUSH":
                with lock:
                    state["flush"] += 1
                state["started"] = False
                state["buffered"] = False
                status("flushed head_unix_ms=%d" % int(time.time() * 1000))
                state["head"] = int(time.time() * 1000)
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
    for path in (raw_path, own_raw):
        if path:
            with open(path, "ab") as f:
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
