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
