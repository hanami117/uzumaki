//! The WAV the device gets: the capture (s16 stereo 44.1 kHz) converted to a fatter format.
//!
//! Why fatter: the Android TV receiver (CrKey 1.56) starts a progressive stream only once it holds
//! a fixed number of **bytes** (live tests 3-4: ~1.36 MB with no length, the FLAC stream took
//! twice as long as the WAV). More bytes per second reach that sooner, so 96 kHz/24-bit (576 KB/s)
//! starts ~3x sooner than 44.1/16 (176 KB/s). 16 → 24 bits is exact; the rate change is a linear
//! interpolation (inaudible below 20 kHz at these rates).

use crate::pcm::RATE;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spec {
    pub rate: u32,
    pub bits: u16,
}

impl Default for Spec {
    fn default() -> Self {
        Spec { rate: 96_000, bits: 24 }
    }
}

impl Spec {
    pub const CAPTURE: Spec = Spec { rate: RATE as u32, bits: 16 };

    /// 44100, 48000, 88200 or 96000 Hz; 16 or 24 bits (what the Cast docs list for LPCM WAV).
    pub fn new(rate: u32, bits: u16) -> Option<Spec> {
        matches!(rate, 44_100 | 48_000 | 88_200 | 96_000).then_some(())?;
        matches!(bits, 16 | 24).then_some(Spec { rate, bits })
    }

    /// "96k/24", "48000/16", "96", "44.1k" ...
    pub fn parse_rate(s: &str) -> Option<u32> {
        let t = s.trim().to_ascii_lowercase();
        let t = t.trim_end_matches("hz");
        let (num, k) = match t.strip_suffix('k') {
            Some(n) => (n, true),
            None => (t, false),
        };
        let v: f64 = num.parse().ok()?;
        let hz = if k || v < 1000.0 { v * 1000.0 } else { v };
        Some(hz.round() as u32)
    }

    pub fn bytes_per_sec(self) -> u64 {
        u64::from(self.rate) * u64::from(self.bits / 8) * 2
    }

    pub fn label(self) -> String {
        format!("{}/{}", if self.rate.is_multiple_of(1000) { format!("{}k", self.rate / 1000) } else { format!("{:.1}k", f64::from(self.rate) / 1000.0) }, self.bits)
    }

    /// A WAV header for a stream of unknown length (sizes 0xFFFFFFFF).
    pub fn header(self) -> Vec<u8> {
        let block = 2 * (self.bits / 8);
        let mut h = Vec::with_capacity(44);
        h.extend_from_slice(b"RIFF");
        h.extend_from_slice(&u32::MAX.to_le_bytes());
        h.extend_from_slice(b"WAVEfmt ");
        h.extend_from_slice(&16u32.to_le_bytes());
        h.extend_from_slice(&1u16.to_le_bytes()); // PCM
        h.extend_from_slice(&2u16.to_le_bytes());
        h.extend_from_slice(&self.rate.to_le_bytes());
        h.extend_from_slice(&(self.bytes_per_sec() as u32).to_le_bytes());
        h.extend_from_slice(&block.to_le_bytes());
        h.extend_from_slice(&self.bits.to_le_bytes());
        h.extend_from_slice(b"data");
        h.extend_from_slice(&u32::MAX.to_le_bytes());
        h
    }
}

/// Streaming conversion of s16le stereo 44.1 kHz bytes to `spec`.
pub struct Converter {
    spec: Spec,
    prev: Option<[i32; 2]>,
    /// Position of the next output frame after `prev`, in input frames (0..1).
    t: f64,
}

impl Converter {
    pub fn new(spec: Spec) -> Self {
        Converter { spec, prev: None, t: 0.0 }
    }

    fn put(&self, out: &mut Vec<u8>, v: i32) {
        if self.spec.bits == 24 {
            let x = v << 8;
            out.extend_from_slice(&x.to_le_bytes()[..3]);
        } else {
            out.extend_from_slice(&(v as i16).to_le_bytes());
        }
    }

    pub fn push(&mut self, pcm: &[u8]) -> Vec<u8> {
        let frames = pcm.as_chunks::<4>().0;
        let mut out = Vec::with_capacity(frames.len() * (self.spec.bytes_per_sec() as usize) / RATE as usize + 16);
        if self.spec.rate as u64 == RATE {
            for f in frames {
                self.put(&mut out, i32::from(i16::from_le_bytes([f[0], f[1]])));
                self.put(&mut out, i32::from(i16::from_le_bytes([f[2], f[3]])));
            }
            return out;
        }
        let step = RATE as f64 / f64::from(self.spec.rate);
        for f in frames {
            let cur = [i32::from(i16::from_le_bytes([f[0], f[1]])), i32::from(i16::from_le_bytes([f[2], f[3]]))];
            let Some(p) = self.prev.replace(cur) else { continue };
            while self.t < 1.0 {
                for ch in 0..2 {
                    let v = f64::from(p[ch]) + f64::from(cur[ch] - p[ch]) * self.t;
                    self.put(&mut out, v.round() as i32);
                }
                self.t += step;
            }
            self.t -= 1.0;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm(frames: &[(i16, i16)]) -> Vec<u8> {
        frames.iter().flat_map(|(l, r)| [l.to_le_bytes(), r.to_le_bytes()].concat()).collect()
    }

    #[test]
    fn specs_and_headers() {
        assert_eq!(Spec::default().bytes_per_sec(), 576_000);
        assert_eq!(Spec::CAPTURE.bytes_per_sec(), 176_400);
        assert_eq!(Spec::new(96_000, 24), Some(Spec::default()));
        assert_eq!(Spec::new(192_000, 24), None);
        assert_eq!(Spec::new(48_000, 32), None);
        assert_eq!(Spec::parse_rate("96k"), Some(96_000));
        assert_eq!(Spec::parse_rate("44.1k"), Some(44_100));
        assert_eq!(Spec::parse_rate("48000"), Some(48_000));
        assert_eq!(Spec::parse_rate("88.2"), Some(88_200));
        assert_eq!(Spec::default().label(), "96k/24");
        let h = Spec::default().header();
        assert_eq!(h.len(), 44);
        assert_eq!(u32::from_le_bytes(h[24..28].try_into().unwrap()), 96_000);
        assert_eq!(u32::from_le_bytes(h[28..32].try_into().unwrap()), 576_000);
        assert_eq!(u16::from_le_bytes([h[32], h[33]]), 6);
        assert_eq!(u16::from_le_bytes([h[34], h[35]]), 24);
    }

    #[test]
    fn sixteen_to_twenty_four_bits_is_exact() {
        let mut c = Converter::new(Spec { rate: 44_100, bits: 24 });
        let out = c.push(&pcm(&[(1, -1), (32767, -32768)]));
        assert_eq!(out, vec![0, 1, 0, 0, 0xff, 0xff, 0, 0xff, 0x7f, 0, 0, 0x80]);
        // Unchanged at 44.1/16.
        let mut same = Converter::new(Spec::CAPTURE);
        let input = pcm(&[(5, 6), (-7, 8)]);
        assert_eq!(same.push(&input), input);
    }

    #[test]
    fn the_rate_change_keeps_time_and_shape() {
        // One second in odd pieces: 96000 frames out (± one).
        let input: Vec<(i16, i16)> = (0..44_100).map(|i| (((i as f64 / 44_100.0 * 440.0 * std::f64::consts::TAU).sin() * 10_000.0) as i16, 0)).collect();
        let bytes = pcm(&input);
        let mut c = Converter::new(Spec { rate: 96_000, bits: 16 });
        let mut out = Vec::new();
        for piece in bytes.chunks(4 * 777) {
            out.extend(c.push(piece));
        }
        let frames = out.len() / 4;
        assert!(frames.abs_diff(96_000) <= 2, "{frames}");
        // The tone is still there: the 96 kHz signal at time t matches the 44.1 kHz one.
        let at96 = |i: usize| i16::from_le_bytes([out[i * 4], out[i * 4 + 1]]);
        for &t in &[0.1, 0.25, 0.5, 0.9] {
            let a = f64::from(at96((t * 96_000.0) as usize));
            let b = (t * 440.0 * std::f64::consts::TAU).sin() * 10_000.0;
            assert!((a - b).abs() < 400.0, "{t}: {a} vs {b}");
        }
    }
}
