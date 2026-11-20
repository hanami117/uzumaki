//! A small streaming FLAC encoder for the live stream: 16-bit stereo 44.1 kHz, blocks of 4096
//! frames, fixed predictors (order 0-4) and Rice-coded residuals, the best of independent /
//! left-side / right-side / mid-side stereo per block. Lossless; about half the size of the WAV on
//! music. The stream header says "length unknown" (total samples 0, no MD5), as a live stream must.

pub const BLOCK: usize = 4096;
const RATE: u32 = 44100;

/// `fLaC` and the one metadata block (STREAMINFO, marked last).
pub fn header() -> Vec<u8> {
    let mut w = Bits::default();
    w.bytes(b"fLaC");
    w.put(1, 1); // last metadata block
    w.put(0, 7); // STREAMINFO
    w.put(34, 24);
    w.put(BLOCK as u64, 16);
    w.put(BLOCK as u64, 16);
    w.put(0, 24); // min frame size: unknown
    w.put(0, 24); // max frame size: unknown
    w.put(u64::from(RATE), 20);
    w.put(1, 3); // channels - 1
    w.put(15, 5); // bits per sample - 1
    w.put(0, 36); // total samples: unknown (live)
    for _ in 0..16 {
        w.put(0, 8); // no MD5
    }
    w.finish()
}

/// Cuts interleaved s16 stereo into blocks and encodes each.
#[derive(Default)]
pub struct Encoder {
    pending: Vec<i16>,
    frame: u64,
}

impl Encoder {
    /// Whole FLAC frames for the samples so far; the rest waits for the next call.
    pub fn push(&mut self, samples: &[i16]) -> Vec<u8> {
        self.pending.extend_from_slice(samples);
        let mut out = Vec::new();
        let per = BLOCK * 2;
        let mut at = 0;
        while self.pending.len() - at >= per {
            out.extend(encode_frame(&self.pending[at..at + per], self.frame));
            self.frame += 1;
            at += per;
        }
        self.pending.drain(..at);
        out
    }
}

#[derive(Default)]
struct Bits {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    /// The low `bits` bits of `v` (at most 32 at once).
    fn put(&mut self, v: u64, bits: u32) {
        if bits > 32 {
            self.put(v >> 32, bits - 32);
            return self.put(v & 0xffff_ffff, 32);
        }
        if bits == 0 {
            return;
        }
        self.acc = (self.acc << bits) | (v & ((1u64 << bits) - 1));
        self.n += bits;
        while self.n >= 8 {
            self.n -= 8;
            self.out.push((self.acc >> self.n) as u8);
        }
        self.acc &= (1u64 << self.n) - 1;
    }

    fn signed(&mut self, v: i32, bits: u32) {
        self.put(u64::from(v as u32), bits);
    }

    fn zeros(&mut self, mut n: u64) {
        while n >= 32 {
            self.put(0, 32);
            n -= 32;
        }
        self.put(0, n as u32);
    }

    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.put(u64::from(x), 8);
        }
    }

    fn len(&self) -> usize {
        self.out.len()
    }

    /// Pads to a byte with zeros.
    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            let pad = 8 - self.n;
            self.put(0, pad);
        }
        self.out
    }
}

fn crc8(b: &[u8]) -> u8 {
    let mut c = 0u8;
    for &x in b {
        c ^= x;
        for _ in 0..8 {
            c = if c & 0x80 != 0 { (c << 1) ^ 0x07 } else { c << 1 };
        }
    }
    c
}

fn crc16(b: &[u8]) -> u16 {
    let mut c = 0u16;
    for &x in b {
        c ^= u16::from(x) << 8;
        for _ in 0..8 {
            c = if c & 0x8000 != 0 { (c << 1) ^ 0x8005 } else { c << 1 };
        }
    }
    c
}

/// FLAC's UTF-8-like coding of the frame number.
fn utf8_number(w: &mut Bits, v: u64) {
    if v < 0x80 {
        return w.put(v, 8);
    }
    let mut n = 2;
    while n < 7 && v >= 1u64 << (5 * n + 1) {
        n += 1;
    }
    let lead: u64 = (0xff00u64 >> n) & 0xff;
    w.put(lead | (v >> (6 * (n - 1))), 8);
    for i in (0..n - 1).rev() {
        w.put(0x80 | ((v >> (6 * i)) & 0x3f), 8);
    }
}

fn residual(x: &[i32], order: usize, out: &mut Vec<i32>) {
    out.clear();
    for i in order..x.len() {
        let e = match order {
            0 => x[i],
            1 => x[i] - x[i - 1],
            2 => x[i] - 2 * x[i - 1] + x[i - 2],
            3 => x[i] - 3 * x[i - 1] + 3 * x[i - 2] - x[i - 3],
            _ => x[i] - 4 * x[i - 1] + 6 * x[i - 2] - 4 * x[i - 3] + x[i - 4],
        };
        out.push(e);
    }
}

fn zigzag(e: i32) -> u32 {
    ((e << 1) ^ (e >> 31)) as u32
}

/// The best predictor order (smallest sum of |residual|) and that sum.
fn best_order(x: &[i32], scratch: &mut Vec<i32>) -> (usize, u64) {
    (0..=4)
        .map(|o| {
            residual(x, o, scratch);
            (o, scratch.iter().map(|e| u64::from(e.unsigned_abs())).sum::<u64>())
        })
        .min_by_key(|&(_, s)| s)
        .unwrap_or((0, 0))
}

/// Rice parameter and its cost in bits for one partition.
fn rice_cost(u: &[u32]) -> (u32, u64) {
    let n = u.len() as u64;
    if n == 0 {
        return (0, 0);
    }
    let sum: u64 = u.iter().map(|&v| u64::from(v)).sum();
    let mean = sum / n;
    let guess = if mean == 0 { 0 } else { 63 - mean.leading_zeros() };
    let mut best = (0, u64::MAX);
    let hi = (guess + 1).min(14);
    for k in guess.saturating_sub(1).min(hi)..=hi {
        let bits = n * u64::from(k + 1) + u.iter().map(|&v| u64::from(v >> k)).sum::<u64>();
        if bits < best.1 {
            best = (k, bits);
        }
    }
    best
}

/// Writes one subframe of `x` (`bps` bits per sample), fixed predictor or verbatim, whichever is smaller.
fn subframe(w: &mut Bits, x: &[i32], bps: u32) {
    let mut scratch = Vec::with_capacity(x.len());
    let (order, _) = best_order(x, &mut scratch);
    residual(x, order, &mut scratch);
    let u: Vec<u32> = scratch.iter().map(|&e| zigzag(e)).collect();
    // Partition order: the cheapest of 0..=6 (each partition holds BLOCK >> p samples, the first
    // `order` fewer).
    let n = x.len();
    let mut plan: Option<(u32, Vec<u32>, u64)> = None;
    for p in 0..=6u32 {
        let part = n >> p;
        if part <= order || !n.is_multiple_of(1 << p) {
            break;
        }
        let mut ks = Vec::new();
        let mut bits = 0u64;
        let mut at = 0;
        for i in 0..(1usize << p) {
            let len = if i == 0 { part - order } else { part };
            let (k, b) = rice_cost(&u[at..at + len]);
            ks.push(k);
            bits += b + 4;
            at += len;
        }
        if plan.as_ref().is_none_or(|(_, _, best)| bits < *best) {
            plan = Some((p, ks, bits));
        }
    }
    let verbatim = (n as u64) * u64::from(bps);
    let Some((p, ks, bits)) = plan.filter(|(_, _, b)| b + (order as u64) * u64::from(bps) + 6 < verbatim) else {
        w.put(0b0000_0010, 8); // pad 0, VERBATIM (000001), no wasted bits
        for &s in x {
            w.signed(s, bps);
        }
        return;
    };
    let _ = bits;
    w.put(0, 1);
    w.put(0b001000 | order as u64, 6); // FIXED, order
    w.put(0, 1);
    for &s in &x[..order] {
        w.signed(s, bps);
    }
    w.put(0, 2); // Rice, 4-bit parameters
    w.put(u64::from(p), 4);
    let part = n >> p;
    let mut at = 0;
    for (i, &k) in ks.iter().enumerate() {
        let len = if i == 0 { part - order } else { part };
        w.put(u64::from(k), 4);
        for &v in &u[at..at + len] {
            w.zeros(u64::from(v >> k));
            w.put(1, 1);
            w.put(u64::from(v), k);
        }
        at += len;
    }
}

/// One frame of `BLOCK` interleaved stereo frames.
fn encode_frame(interleaved: &[i16], number: u64) -> Vec<u8> {
    let n = interleaved.len() / 2;
    let l: Vec<i32> = interleaved.iter().step_by(2).map(|&s| i32::from(s)).collect();
    let r: Vec<i32> = interleaved.iter().skip(1).step_by(2).map(|&s| i32::from(s)).collect();
    let side: Vec<i32> = l.iter().zip(&r).map(|(a, b)| a - b).collect();
    let mid: Vec<i32> = l.iter().zip(&r).map(|(a, b)| (a + b) >> 1).collect();
    let mut scratch = Vec::with_capacity(n);
    let [cl, cr, cs, cm] = [&l, &r, &side, &mid].map(|x| best_order(x, &mut scratch).1);
    // Channel assignment: 1 = independent, 8 = left/side, 9 = side/right, 10 = mid/side.
    let (assign, a, b, bps_a, bps_b) = [(1u64, &l, &r, 16, 16, cl + cr), (8, &l, &side, 16, 17, cl + cs), (9, &side, &r, 17, 16, cs + cr), (10, &mid, &side, 16, 17, cm + cs)]
        .into_iter()
        .min_by_key(|t| t.5)
        .map(|t| (t.0, t.1, t.2, t.3, t.4))
        .unwrap();
    let mut w = Bits::default();
    w.put(0b11_1111_1111_1110, 14);
    w.put(0, 1); // reserved
    w.put(0, 1); // fixed block size
    w.put(if n == BLOCK { 0b1100 } else { 0b0111 }, 4);
    w.put(0b1001, 4); // 44.1 kHz
    w.put(assign, 4);
    w.put(0b100, 3); // 16 bits per sample
    w.put(0, 1);
    utf8_number(&mut w, number);
    if n != BLOCK {
        w.put((n - 1) as u64, 16);
    }
    let crc = crc8(&w.out);
    debug_assert_eq!(w.n, 0);
    w.put(u64::from(crc), 8);
    let _ = w.len();
    subframe(&mut w, a, bps_a);
    subframe(&mut w, b, bps_b);
    let mut out = w.finish();
    let crc = crc16(&out);
    out.extend_from_slice(&crc.to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    fn decode(bytes: Vec<u8>) -> Vec<i16> {
        let mss = MediaSourceStream::new(Box::new(std::io::Cursor::new(bytes)), Default::default());
        let mut hint = Hint::new();
        hint.with_extension("flac");
        let probed = symphonia::default::get_probe().format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default()).unwrap();
        let mut format = probed.format;
        let track = format.tracks().iter().find(|t| t.codec_params.codec != CODEC_TYPE_NULL).unwrap().clone();
        let mut dec = symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions { verify: true }).unwrap();
        let mut out = Vec::new();
        while let Ok(packet) = format.next_packet() {
            let buf = dec.decode(&packet).unwrap();
            let mut s = SampleBuffer::<i16>::new(buf.capacity() as u64, *buf.spec());
            s.copy_interleaved_ref(buf);
            out.extend_from_slice(s.samples());
        }
        out
    }

    /// Music-like test signal: two tones, a little noise, a loud clipped part and silence.
    fn signal(frames: usize) -> Vec<i16> {
        let mut seed = 12345u32;
        let mut noise = || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            ((seed >> 16) as i32 % 600) - 300
        };
        let mut v = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let t = i as f64 / 44100.0;
            let (a, b) = if i > frames * 3 / 4 {
                (0, 0)
            } else if i > frames / 2 {
                (if (i / 50) % 2 == 0 { 32767 } else { -32768 }, -32768 + (i % 65536) as i32)
            } else {
                let x = (t * 440.0 * std::f64::consts::TAU).sin() * 9000.0 + (t * 97.0 * std::f64::consts::TAU).sin() * 4000.0;
                (x as i32 + noise(), (x * 0.7) as i32 - noise())
            };
            v.push(a.clamp(-32768, 32767) as i16);
            v.push(b.clamp(-32768, 32767) as i16);
        }
        v
    }

    #[test]
    fn the_stream_decodes_back_bit_exact() {
        let input = signal(BLOCK * 12);
        let mut enc = Encoder::default();
        let mut bytes = header();
        // In uneven pieces, as the PCM arrives.
        for piece in input.chunks(2 * 1000 + 6) {
            bytes.extend(enc.push(piece));
        }
        let wav = input.len() * 2;
        assert!(bytes.len() < wav * 3 / 4, "compresses: {} of {} bytes", bytes.len(), wav);
        assert_eq!(decode(bytes), input);
    }

    #[test]
    fn large_frame_numbers_are_coded_as_flac_wants() {
        let mut w = Bits::default();
        utf8_number(&mut w, 0x7f);
        utf8_number(&mut w, 0x80);
        utf8_number(&mut w, 0x7ff);
        utf8_number(&mut w, 0x800);
        assert_eq!(w.finish(), vec![0x7f, 0xc2, 0x80, 0xdf, 0xbf, 0xe0, 0xa0, 0x80]);
        // A stream well past 2^16 frames (~ 1.7 hours) still decodes.
        let input = signal(BLOCK * 2);
        let mut bytes = header();
        let mut enc = Encoder { frame: 70_000, ..Encoder::default() };
        bytes.extend(enc.push(&input));
        assert_eq!(decode(bytes), input);
    }
}
