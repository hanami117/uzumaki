//! The two HLS playlists of a motion clip: the master (one line per variant) and the chosen
//! variant's media playlist. Apple's clips are fMP4 with every segment a byte range of one
//! `.mp4` file, so a variant is a single download. Anything encrypted (`EXT-X-KEY`,
//! `EXT-X-SESSION-KEY`) is refused: those clips are never shown.

/// One `EXT-X-STREAM-INF` line of a master playlist and the URI under it.
#[derive(Clone, Debug, PartialEq)]
pub struct Variant {
    pub uri: String,
    pub width: u32,
    pub height: u32,
    /// `avc1…` (H.264) or `hvc1…` (HEVC), as listed.
    pub codecs: String,
    pub bandwidth: u64,
    /// Frames per second, when listed.
    pub fps: Option<f64>,
}

impl Variant {
    pub fn is_h264(&self) -> bool {
        self.codecs.starts_with("avc1")
    }
    fn is_square(&self) -> bool {
        self.width > 0 && self.width == self.height
    }
}

/// The size we want: sharp at the Now Playing cover's usual 400 to 600 pt on a 1× screen, and
/// cheap to decode (the spike's measurements). The nearest square variant wins otherwise.
pub const TARGET_SIDE: u32 = 768;

fn refuses(text: &str) -> bool {
    text.lines().any(|l| {
        let l = l.trim_start();
        l.starts_with("#EXT-X-KEY") || l.starts_with("#EXT-X-SESSION-KEY")
    })
}

/// Splits an attribute list (`A=1,B="x,y",C=2`) into pairs, quotes removed.
fn attrs(list: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let mut rest = list;
    while !rest.is_empty() {
        let Some(eq) = rest.find('=') else { break };
        let key = rest[..eq].trim();
        rest = &rest[eq + 1..];
        let value;
        if let Some(r) = rest.strip_prefix('"') {
            let end = r.find('"').unwrap_or(r.len());
            value = &r[..end];
            rest = r.get(end + 1..).unwrap_or("");
            rest = rest.strip_prefix(',').unwrap_or(rest);
        } else {
            let end = rest.find(',').unwrap_or(rest.len());
            value = &rest[..end];
            rest = rest.get(end + 1..).unwrap_or("");
        }
        out.push((key, value));
    }
    out
}

/// `uri` made absolute against the playlist it came from.
pub fn resolve(base: &str, uri: &str) -> String {
    if uri.contains("://") {
        return uri.to_string();
    }
    let base = base.split(['?', '#']).next().unwrap_or(base);
    if let Some(path) = uri.strip_prefix('/') {
        // Scheme and host of `base`, then the absolute path.
        let host_end = base.find("://").map(|i| i + 3).and_then(|i| base[i..].find('/').map(|j| i + j)).unwrap_or(base.len());
        return format!("{}/{path}", &base[..host_end]);
    }
    match base.rfind('/') {
        Some(i) => format!("{}/{uri}", &base[..i]),
        None => uri.to_string(),
    }
}

/// The variants of a master playlist (I-frame-only playlists left out). Err when it isn't a
/// playlist or carries a key.
pub fn parse_master(text: &str, base: &str) -> Result<Vec<Variant>, String> {
    if !text.trim_start().starts_with("#EXTM3U") {
        return Err("not an HLS playlist".into());
    }
    if refuses(text) {
        return Err("encrypted (EXT-X-KEY)".into());
    }
    let mut out = Vec::new();
    let mut pending: Option<Variant> = None;
    for line in text.lines().map(str::trim) {
        if let Some(list) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            let mut v = Variant { uri: String::new(), width: 0, height: 0, codecs: String::new(), bandwidth: 0, fps: None };
            for (k, val) in attrs(list) {
                match k {
                    "RESOLUTION" => {
                        if let Some((w, h)) = val.split_once('x') {
                            v.width = w.parse().unwrap_or(0);
                            v.height = h.parse().unwrap_or(0);
                        }
                    }
                    "CODECS" => v.codecs = val.to_string(),
                    "AVERAGE-BANDWIDTH" => v.bandwidth = val.parse().unwrap_or(v.bandwidth),
                    "BANDWIDTH" if v.bandwidth == 0 => v.bandwidth = val.parse().unwrap_or(0),
                    "FRAME-RATE" => v.fps = val.parse().ok().filter(|f: &f64| f.is_finite() && *f > 0.0),
                    _ => {}
                }
            }
            pending = Some(v);
        } else if !line.is_empty() && !line.starts_with('#') && let Some(mut v) = pending.take() {
            v.uri = resolve(base, line);
            out.push(v);
        }
    }
    Ok(out)
}

/// The variant to download: square, H.264 at `TARGET_SIDE` (the cheapest to decode everywhere,
/// no 10-bit), else the square variant nearest that size (H.264 before HEVC, then the lower
/// bit rate). None when there is no square variant.
pub fn choose(variants: &[Variant]) -> Option<&Variant> {
    variants
        .iter()
        .filter(|v| v.is_square())
        .min_by_key(|v| (v.width.abs_diff(TARGET_SIDE), !v.is_h264(), v.width < TARGET_SIDE, v.bandwidth))
}

/// The one file a variant's media playlist plays, made absolute. Err when it is encrypted,
/// empty, or spread over several files (not what Apple serves for these clips; not handled).
pub fn media_file(text: &str, base: &str) -> Result<String, String> {
    if !text.trim_start().starts_with("#EXTM3U") {
        return Err("not an HLS playlist".into());
    }
    if refuses(text) {
        return Err("encrypted (EXT-X-KEY)".into());
    }
    let mut files: Vec<String> = Vec::new();
    let mut add = |uri: &str| {
        let abs = resolve(base, uri);
        if !files.contains(&abs) {
            files.push(abs);
        }
    };
    for line in text.lines().map(str::trim) {
        if let Some(list) = line.strip_prefix("#EXT-X-MAP:") {
            if let Some((_, uri)) = attrs(list).into_iter().find(|(k, _)| *k == "URI") {
                add(uri);
            }
        } else if !line.is_empty() && !line.starts_with('#') {
            add(line);
        }
    }
    match files.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err("no media file".into()),
        _ => Err(format!("{} media files (segmented clips aren't handled)", files.len())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped like Apple's master for an album's `motionDetailSquare` (2026-11-13), trimmed.
    const MASTER: &str = r#"#EXTM3U
#EXT-X-VERSION:7
#EXT-X-INDEPENDENT-SEGMENTS

#EXT-X-I-FRAME-STREAM-INF:AVERAGE-BANDWIDTH=182179,BANDWIDTH=192705,CODECS="hvc1.2.20000000.L123.B0",RESOLUTION=768x768,URI="https://cdn.example/a/P1_trickPlay_gr540_sdr_768x768_iframes.m3u8"

#EXT-X-STREAM-INF:AVERAGE-BANDWIDTH=2033235,BANDWIDTH=2236103,VIDEO-RANGE=SDR,CLOSED-CAPTIONS=NONE,CODECS="avc1.64001f",FRAME-RATE=23.976,RESOLUTION=768x768,STABLE-VARIANT-ID="beff"
https://cdn.example/b/P1_Anull_video_gr240_sdr_768x768.m3u8
#EXT-X-STREAM-INF:AVERAGE-BANDWIDTH=1089592,BANDWIDTH=1187856,VIDEO-RANGE=SDR,CLOSED-CAPTIONS=NONE,CODECS="hvc1.2.20000000.L123.B0",FRAME-RATE=23.976,RESOLUTION=768x768
https://cdn.example/c/P1_Anull_video_gr540_sdr_768x768.m3u8
#EXT-X-STREAM-INF:AVERAGE-BANDWIDTH=251188,BANDWIDTH=269842,CODECS="avc1.64001f",FRAME-RATE=23.976,RESOLUTION=360x360
https://cdn.example/d/P1_Anull_video_gr180_sdr_360x360.m3u8
#EXT-X-STREAM-INF:AVERAGE-BANDWIDTH=2623047,BANDWIDTH=2848870,CODECS="avc1.64001f",FRAME-RATE=23.976,RESOLUTION=960x960
https://cdn.example/e/P1_Anull_video_gr250_sdr_960x960.m3u8
#EXT-X-STREAM-INF:AVERAGE-BANDWIDTH=7089019,BANDWIDTH=7776588,CODECS="hvc1.2.20000000.H150.B0",FRAME-RATE=23.976,RESOLUTION=1920x1920
https://cdn.example/f/P1_Anull_video_gr700_sdr_1920x1920.m3u8
"#;

    const MEDIA: &str = r#"#EXTM3U
#EXT-X-TARGETDURATION:9
#EXT-X-VERSION:7
#EXT-X-MEDIA-SEQUENCE:0
#EXT-X-PLAYLIST-TYPE:VOD
#EXT-X-INDEPENDENT-SEGMENTS
#EXT-X-MAP:URI="P1_Anull_video_gr240_sdr_768x768-.mp4",BYTERANGE="897@0"
#EXTINF:8.71704,
#EXT-X-BYTERANGE:1981909@897
P1_Anull_video_gr240_sdr_768x768-.mp4
#EXTINF:7.50750,
#EXT-X-BYTERANGE:2098442@1982806
P1_Anull_video_gr240_sdr_768x768-.mp4
#EXT-X-ENDLIST
"#;

    const BASE: &str = "https://cdn.example/m/P1_default.m3u8";

    #[test]
    fn the_master_lists_its_variants_without_the_iframe_playlists() {
        let vs = parse_master(MASTER, BASE).unwrap();
        assert_eq!(vs.len(), 5);
        assert_eq!(vs[0].uri, "https://cdn.example/b/P1_Anull_video_gr240_sdr_768x768.m3u8");
        assert_eq!((vs[0].width, vs[0].height), (768, 768));
        assert_eq!(vs[0].codecs, "avc1.64001f");
        assert_eq!(vs[0].bandwidth, 2033235, "the average, not the peak");
        assert_eq!(vs[0].fps, Some(23.976));
        assert!(vs[0].is_h264() && !vs[1].is_h264());
    }

    #[test]
    fn picks_the_768_h264_variant() {
        let vs = parse_master(MASTER, BASE).unwrap();
        assert_eq!(choose(&vs).unwrap().uri, "https://cdn.example/b/P1_Anull_video_gr240_sdr_768x768.m3u8");
    }

    #[test]
    fn without_768_h264_takes_the_nearest_square_variant() {
        let mut vs = parse_master(MASTER, BASE).unwrap();
        vs.retain(|v| !v.uri.contains("/b/"));
        // HEVC 768 is nearer than H.264 960 or 360.
        assert!(choose(&vs).unwrap().uri.contains("/c/"));
        vs.retain(|v| !v.uri.contains("/c/"));
        // Then the nearest size: 960 (192 away) beats 360 (408 away) and 1920.
        assert!(choose(&vs).unwrap().uri.contains("/e/"));
        // Ties go to H.264, then to the larger picture.
        let tie = vec![
            Variant { uri: "small".into(), width: 576, height: 576, codecs: "avc1".into(), bandwidth: 1, fps: None },
            Variant { uri: "big".into(), width: 960, height: 960, codecs: "avc1".into(), bandwidth: 9, fps: None },
            Variant { uri: "hevc".into(), width: 960, height: 960, codecs: "hvc1".into(), bandwidth: 1, fps: None },
        ];
        assert_eq!(choose(&tie).unwrap().uri, "big");
    }

    #[test]
    fn tall_or_wide_variants_are_never_chosen() {
        let tall = vec![Variant { uri: "t".into(), width: 768, height: 1024, codecs: "avc1".into(), bandwidth: 1, fps: None }];
        assert!(choose(&tall).is_none());
        assert!(choose(&[]).is_none());
    }

    #[test]
    fn a_key_anywhere_refuses_the_clip() {
        let keyed = MASTER.replace("#EXT-X-INDEPENDENT-SEGMENTS", "#EXT-X-SESSION-KEY:METHOD=SAMPLE-AES,URI=\"skd://x\"");
        assert!(parse_master(&keyed, BASE).is_err());
        let keyed = MEDIA.replace("#EXT-X-INDEPENDENT-SEGMENTS", "#EXT-X-KEY:METHOD=AES-128,URI=\"k\"");
        assert!(media_file(&keyed, BASE).is_err());
        assert!(parse_master("<html>", BASE).is_err());
    }

    #[test]
    fn the_media_playlist_is_one_file_resolved_against_its_playlist() {
        let base = "https://cdn.example/b/P1_Anull_video_gr240_sdr_768x768.m3u8";
        assert_eq!(media_file(MEDIA, base).unwrap(), "https://cdn.example/b/P1_Anull_video_gr240_sdr_768x768-.mp4");
        let two = MEDIA.replacen("P1_Anull_video_gr240_sdr_768x768-.mp4\n#EXT-X-ENDLIST", "other.mp4\n#EXT-X-ENDLIST", 1);
        assert!(media_file(&two, base).is_err());
    }

    #[test]
    fn relative_and_absolute_uris_resolve() {
        assert_eq!(resolve("https://h.example/a/b/m.m3u8?x=1", "v.mp4"), "https://h.example/a/b/v.mp4");
        assert_eq!(resolve("https://h.example/a/b/m.m3u8", "/root/v.mp4"), "https://h.example/root/v.mp4");
        assert_eq!(resolve("https://h.example/a/m.m3u8", "https://o.example/v.mp4"), "https://o.example/v.mp4");
    }
}
