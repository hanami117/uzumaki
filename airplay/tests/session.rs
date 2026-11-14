//! The session against a stand-in for cliairplay (a shell script): no network, no speaker, no audio
//! device. The test plays the bridge: it connects to the PCM socket and writes PCM.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use uzumaki_airplay::{Config, Spike, helper::Options};

const FAKE: &str = r#"#!/bin/sh
# Stand-in for cliairplay: the status lines the session waits for, and a note of what it got.
out="$(dirname "$0")/fake.txt"
prev=""; for a in "$@"; do [ "$prev" = "--cmdpipe" ] && pipe="$a"; prev="$a"; done
echo "args: $*" > "$out"
echo "[STATUS] route protocol=airplay2 flow=native timing=ptp buffered=0"
echo "[STATUS] connected" >&2
exec 3<>"$pipe"
n=$(dd bs=4096 count=2 2>/dev/null | wc -c | tr -d ' ')
echo "pcm: $n" >> "$out"
echo "[STATUS] audio buffered_ms=46" >&2
read -r l1 <&3; read -r l2 <&3
echo "cmd: $l1 $l2" >> "$out"
echo "[STATUS] started requested_unix_ms=0 at_unix_ms=1" >&2
cat > /dev/null
"#;

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn bridge_pcm_reaches_the_helper_and_the_speaker_is_started() {
    let dir = std::env::temp_dir().join(format!("uzu-airplay-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let helper = dir.join("fake-cliairplay");
    std::fs::write(&helper, FAKE).unwrap();
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cfg = Config {
        target: "Bedroom".into(),
        host: Some(("127.0.0.1".parse().unwrap(), 7000)),
        helper: Some(helper),
        options: Options { volume: 30, debug: None, extra: vec![], dacp: "0123456789ABCDEF".into(), active_remote: 1 },
        dir: dir.join("run"),
        discover_timeout: Duration::from_secs(1),
        retry_delay: Duration::from_millis(100),
        keep_local: false,
    };
    let spike = Spike::start(cfg).unwrap();
    let socket: PathBuf = spike.socket().to_path_buf();
    // The socket exists only once the helper says it is connected.
    let mut bridge = None;
    wait_for("the PCM socket", || {
        bridge = UnixStream::connect(&socket).ok();
        bridge.is_some()
    });
    // A whole-frame chunk of s16le stereo, as the bridge sends.
    let chunk: Vec<u8> = (0..4096 * 4).map(|i| (i % 251) as u8).collect();
    bridge.as_mut().unwrap().write_all(&chunk).unwrap();
    let note = dir.join("fake.txt");
    wait_for("START on the command pipe", || std::fs::read_to_string(&note).is_ok_and(|s| s.contains("cmd:")));
    let got = std::fs::read_to_string(&note).unwrap();
    assert!(got.contains("pcm: 8192"), "{got}");
    assert!(got.contains("cmd: START_UNIX_MS=0 ACTION=START"), "{got}");
    assert!(got.contains("--protocol auto") && got.contains("--volume 30") && got.trim_end().lines().next().unwrap().ends_with("127.0.0.1"), "{got}");
    spike.stop();
    wait_for("the socket to go", || !socket.exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Stand-in that records everything it gets after START, raw, into pcm.bin (a test file in a
/// private temp folder, removed at the end).
const RECORDER: &str = r#"#!/bin/sh
dir="$(dirname "$0")"
prev=""; for a in "$@"; do [ "$prev" = "--cmdpipe" ] && pipe="$a"; prev="$a"; done
echo "started once" >> "$dir/runs.txt"
echo "[STATUS] connected" >&2
exec 3<>"$pipe"
echo "[STATUS] audio buffered_ms=46" >&2
read -r l1 <&3; read -r l2 <&3
echo "[STATUS] started requested_unix_ms=0 at_unix_ms=1" >&2
cat > "$dir/pcm.bin"
"#;

fn samples(bytes: &[u8]) -> Vec<i16> {
    bytes.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b)).collect()
}

#[test]
fn one_session_serves_the_bridge_and_the_local_player_in_turn() {
    let dir = std::env::temp_dir().join(format!("uzu-airplay-switch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let helper = dir.join("fake-cliairplay");
    std::fs::write(&helper, RECORDER).unwrap();
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cfg = Config {
        target: "Bedroom".into(),
        host: Some(("127.0.0.1".parse().unwrap(), 7000)),
        helper: Some(helper),
        options: Options { volume: 30, debug: None, extra: vec![], dacp: "0123456789ABCDEF".into(), active_remote: 1 },
        dir: dir.join("run"),
        discover_timeout: Duration::from_secs(1),
        retry_delay: Duration::from_millis(100),
        keep_local: false,
    };
    let spike = Spike::start(cfg).unwrap();
    let local = spike.local_feed();
    assert!(!local.mute_local(), "no speaker yet: the Mac plays");
    let socket: PathBuf = spike.socket().to_path_buf();
    let mut bridge = None;
    wait_for("the PCM socket", || {
        bridge = UnixStream::connect(&socket).ok();
        bridge.is_some()
    });
    let mut bridge = bridge.unwrap();
    assert!(local.casting() && local.mute_local(), "connected: the Mac goes quiet");
    let pcm = dir.join("pcm.bin");
    let got = || samples(&std::fs::read(&pcm).unwrap_or_default());

    // 1. Apple Music plays: the bridge's samples (0x1111) arrive, also when split mid-frame.
    let a: Vec<u8> = std::iter::repeat_n(0x1111i16.to_le_bytes(), 4096 * 2).flatten().collect();
    bridge.write_all(&a[..4097]).unwrap();
    bridge.write_all(&a[4097..]).unwrap();
    wait_for("the bridge's audio", || got().iter().filter(|&&s| s == 0x1111).count() == 8192);

    // 2. A local file takes over: its samples arrive, the bridge's (paused, 0x3333) are dropped.
    local.set_active(true);
    local.push(vec![0x2222; 4096 * 2]);
    wait_for("the local audio", || got().iter().filter(|&&s| s == 0x2222).count() == 8192);
    bridge.write_all(&std::iter::repeat_n(0x3333i16.to_le_bytes(), 4096 * 2).flatten().collect::<Vec<u8>>()).unwrap();
    // 3. Local paused: silence keeps the stream going.
    let before = got().len();
    wait_for("silence while the local player is paused", || got().len() >= before + 2 * 4410);
    assert!(got()[before..].iter().all(|&s| s == 0), "only silence while paused");

    // 4. Back to Apple Music on the same session.
    local.set_active(false);
    std::thread::sleep(Duration::from_millis(100));
    bridge.write_all(&std::iter::repeat_n(0x4444i16.to_le_bytes(), 4096 * 2).flatten().collect::<Vec<u8>>()).unwrap();
    wait_for("the bridge's audio again", || got().iter().filter(|&&s| s == 0x4444).count() == 8192);

    let all = got();
    assert!(!all.contains(&0x3333), "the bridge's audio was dropped while the local player was in charge");
    let first = |v: i16| all.iter().position(|&s| s == v).unwrap();
    assert!(first(0x1111) < first(0x2222) && first(0x2222) < first(0x4444), "in order");
    assert_eq!(std::fs::read_to_string(dir.join("runs.txt")).unwrap().lines().count(), 1, "one helper session for both");
    spike.stop();
    wait_for("the socket to go", || !socket.exists());
    assert!(!local.casting() && !local.mute_local(), "stopped: the Mac plays again");
    std::fs::remove_dir_all(&dir).unwrap();
}
