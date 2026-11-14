//! AirPlay spike (`UZUMAKI_AIRPLAY_SPIKE=1` only): a tap on the local player's output.
//!
//! [`Tap`] wraps the decoder inside the rodio sink. The samples it passes on to the sink are the
//! decoder's, unchanged, or silence while a speaker takes the audio (so this computer stays quiet
//! without touching the user's volume). A copy, resampled to stereo 44.1 kHz s16 (what the AirPlay
//! helper takes, as the bridge's capture) and scaled by the app's volume (as MusicKit's volume
//! scales the bridge's capture), goes to a [`CastSink`] in 4096-frame chunks. Without the spike the
//! player doesn't build a `Tap` at all.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use rodio::source::SeekError;
use rodio::{ChannelCount, SampleRate, Source};

/// The helper's input rate.
pub const OUT_RATE: u32 = 44100;
/// Frames per chunk handed to the sink (~93 ms, as the bridge's worklet).
const CHUNK_FRAMES: usize = 4096;

/// Where the tapped audio goes (the AirPlay session; a recorder in tests).
pub trait CastSink: Send + Sync {
    /// A speaker takes audio now: only then is the copy made.
    fn casting(&self) -> bool;
    /// This computer's output must be silent.
    fn mute_local(&self) -> bool;
    /// Interleaved stereo s16 at 44.1 kHz. Must not block (audio thread).
    fn push(&self, chunk: Vec<i16>);
    /// The local player has a song loaded (it, not the bridge, feeds the speaker).
    fn set_active(&self, on: bool);
}

impl CastSink for uzumaki_airplay::LocalFeed {
    fn casting(&self) -> bool {
        uzumaki_airplay::LocalFeed::casting(self)
    }
    fn mute_local(&self) -> bool {
        uzumaki_airplay::LocalFeed::mute_local(self)
    }
    fn push(&self, chunk: Vec<i16>) {
        uzumaki_airplay::LocalFeed::push(self, chunk)
    }
    fn set_active(&self, on: bool) {
        uzumaki_airplay::LocalFeed::set_active(self, on)
    }
}

/// A linear gain shared with the player thread (f32 bits), the app's volume.
#[derive(Clone)]
pub struct Gain(Arc<AtomicU32>);

impl Gain {
    pub fn new(v: f32) -> Self {
        Self(Arc::new(AtomicU32::new(v.to_bits())))
    }
    pub fn set(&self, v: f32) {
        self.0.store(v.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
}

/// Streaming linear-interpolation resampler for stereo frames, any rate → 44.1 kHz.
/// Equal rates are an exact copy (one frame late).
#[derive(Default)]
pub struct Resampler {
    prev: Option<[f32; 2]>,
    /// Position of the next output frame after `prev`, in input frames (0..1).
    t: f64,
}

impl Resampler {
    pub fn reset(&mut self) {
        self.prev = None;
        self.t = 0.0;
    }

    /// Feeds one input frame at `rate`; appends the output frames it completes to `out`.
    pub fn push(&mut self, frame: [f32; 2], rate: u32, gain: f32, out: &mut Vec<i16>) {
        let Some(p) = self.prev.replace(frame) else { return };
        let step = f64::from(rate.max(1)) / f64::from(OUT_RATE);
        while self.t < 1.0 {
            let t = self.t as f32;
            for ch in 0..2 {
                let v = (p[ch] + (frame[ch] - p[ch]) * t) * gain;
                out.push((v.clamp(-1.0, 1.0) * 32767.0).round() as i16);
            }
            self.t += step;
        }
        self.t -= 1.0;
    }
}

/// The decoder, tapped. See the module comment.
pub struct Tap<S: Source> {
    inner: S,
    sink: Arc<dyn CastSink>,
    gain: Gain,
    resampler: Resampler,
    chunk: Vec<i16>,
    /// Position inside the current frame, and the frame's layout and rate (read at its start).
    pos: u16,
    channels: ChannelCount,
    rate: SampleRate,
    frame: [f32; 2],
    /// Read at each frame's start: copy for the speaker / silence for this computer.
    copying: bool,
    muted: bool,
}

impl<S: Source> Tap<S> {
    pub fn new(inner: S, sink: Arc<dyn CastSink>, gain: Gain) -> Self {
        let channels = inner.channels().max(1);
        let rate = inner.sample_rate();
        Self {
            inner,
            sink,
            gain,
            resampler: Resampler::default(),
            chunk: Vec::with_capacity(CHUNK_FRAMES * 2),
            pos: 0,
            channels,
            rate,
            frame: [0.0; 2],
            copying: false,
            muted: false,
        }
    }

    fn flush(&mut self) {
        if !self.chunk.is_empty() {
            let full = std::mem::replace(&mut self.chunk, Vec::with_capacity(CHUNK_FRAMES * 2));
            self.sink.push(full);
        }
    }

    fn frame_start(&mut self) {
        self.channels = self.inner.channels().max(1);
        self.rate = self.inner.sample_rate();
        let copying = self.sink.casting();
        if copying != self.copying {
            self.copying = copying;
            self.resampler.reset();
            self.chunk.clear();
        }
        self.muted = self.sink.mute_local();
    }
}

impl<S: Source> Iterator for Tap<S> {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.pos == 0 {
            self.frame_start();
        }
        let s = self.inner.next()?;
        if self.copying {
            // Stereo out: first two channels; mono goes to both.
            match self.pos {
                0 => self.frame = [s, s],
                1 => self.frame[1] = s,
                _ => {}
            }
        }
        self.pos += 1;
        if self.pos >= self.channels {
            self.pos = 0;
            if self.copying {
                let gain = self.gain.get();
                self.resampler.push(self.frame, self.rate, gain, &mut self.chunk);
                if self.chunk.len() >= CHUNK_FRAMES * 2 {
                    self.flush();
                }
            }
        }
        Some(if self.muted { 0.0 } else { s })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<S: Source> Source for Tap<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }
    fn channels(&self) -> ChannelCount {
        self.inner.channels()
    }
    fn sample_rate(&self) -> SampleRate {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        self.inner.try_seek(pos)?;
        // A seek lands on a frame boundary; don't interpolate across it.
        self.pos = 0;
        self.resampler.reset();
        Ok(())
    }
}

impl<S: Source> Drop for Tap<S> {
    fn drop(&mut self) {
        if self.copying {
            self.flush(); // the end of the song
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use rodio::buffer::SamplesBuffer;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;

    /// A speaker stand-in: the chunks it got, and switches for the test.
    #[derive(Default)]
    pub struct Recorder {
        pub casting: AtomicBool,
        pub keep_local: AtomicBool,
        pub active: AtomicBool,
        pub chunks: Mutex<Vec<Vec<i16>>>,
    }

    impl Recorder {
        pub fn casting() -> Arc<Self> {
            let r = Arc::new(Self::default());
            r.casting.store(true, Ordering::SeqCst);
            r
        }
        pub fn samples(&self) -> Vec<i16> {
            self.chunks.lock().unwrap().concat()
        }
    }

    impl CastSink for Recorder {
        fn casting(&self) -> bool {
            self.casting.load(Ordering::SeqCst)
        }
        fn mute_local(&self) -> bool {
            self.casting() && !self.keep_local.load(Ordering::SeqCst)
        }
        fn push(&self, chunk: Vec<i16>) {
            self.chunks.lock().unwrap().push(chunk);
        }
        fn set_active(&self, on: bool) {
            self.active.store(on, Ordering::SeqCst);
        }
    }

    fn sine(rate: u32, channels: u16, secs: f32, hz: f32, amp: f32) -> SamplesBuffer {
        let n = (rate as f32 * secs) as usize;
        let data: Vec<f32> = (0..n)
            .flat_map(|i| {
                let v = amp * (2.0 * std::f32::consts::PI * hz * i as f32 / rate as f32).sin();
                std::iter::repeat_n(v, channels as usize)
            })
            .collect();
        SamplesBuffer::new(channels, rate, data)
    }

    fn rms(s: &[i16]) -> f64 {
        (s.iter().map(|&v| (v as f64 / 32768.0).powi(2)).sum::<f64>() / s.len() as f64).sqrt()
    }

    /// Zero crossings per second of one channel: a frequency check that survives resampling.
    fn freq(s: &[i16], ch: usize) -> f64 {
        let x: Vec<i16> = s.iter().skip(ch).step_by(2).copied().collect();
        let crossings = x.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count();
        crossings as f64 / 2.0 / (x.len() as f64 / OUT_RATE as f64)
    }

    #[test]
    fn equal_rate_stereo_is_copied_exactly_and_passes_through() {
        let rec = Recorder::casting();
        rec.keep_local.store(true, Ordering::SeqCst); // A/B switch: this computer keeps playing
        let data: Vec<f32> = (0..20000).map(|i| ((i % 200) as f32 - 100.0) / 128.0).collect();
        let tap = Tap::new(SamplesBuffer::new(2, 44100, data.clone()), rec.clone(), Gain::new(1.0));
        let played: Vec<f32> = tap.collect();
        assert_eq!(played, data, "the sink gets the decoder's samples unchanged");
        let got = rec.samples();
        // One frame late (the resampler needs a next frame), otherwise the same samples.
        assert_eq!(got.len(), data.len() - 2);
        for (g, d) in got.iter().zip(&data) {
            assert_eq!(*g, (d.clamp(-1.0, 1.0) * 32767.0).round() as i16);
        }
        // 9998 frames: two full chunks and the rest at the end.
        let sizes: Vec<usize> = rec.chunks.lock().unwrap().iter().map(Vec::len).collect();
        assert_eq!(sizes, vec![8192, 8192, 19998 - 16384]);
    }

    #[test]
    fn resamples_48k_to_44k1_keeping_pitch_and_level() {
        let rec = Recorder::casting();
        let tap = Tap::new(sine(48000, 2, 1.0, 1000.0, 0.5), rec.clone(), Gain::new(1.0));
        assert_eq!(tap.count(), 96000);
        let got = rec.samples();
        let frames = got.len() / 2;
        assert!((frames as i64 - 44100).abs() <= 2, "{frames} frames for one second");
        assert!((freq(&got, 0) - 1000.0).abs() < 5.0, "{} Hz", freq(&got, 0));
        assert!((rms(&got) - 0.5 / 2f64.sqrt()).abs() < 0.01, "rms {}", rms(&got));
    }

    #[test]
    fn upsamples_mono_22k05_to_stereo() {
        let rec = Recorder::casting();
        let tap = Tap::new(sine(22050, 1, 0.5, 440.0, 0.8), rec.clone(), Gain::new(1.0));
        assert_eq!(tap.count(), 11025);
        let got = rec.samples();
        assert!(((got.len() / 2) as i64 - 22050).abs() <= 3, "{}", got.len() / 2);
        assert!(got.as_chunks::<2>().0.iter().all(|f| f[0] == f[1]), "mono goes to both channels");
        assert!((freq(&got, 1) - 440.0).abs() < 5.0);
    }

    #[test]
    fn surround_keeps_the_front_pair() {
        let rec = Recorder::casting();
        // 6 channels: L = 0.25, R = -0.25, the rest loud.
        let data: Vec<f32> = (0..4410 * 6).map(|i| [0.25, -0.25, 0.9, 0.9, 0.9, 0.9][i % 6]).collect();
        Tap::new(SamplesBuffer::new(6, 44100, data), rec.clone(), Gain::new(1.0)).for_each(drop);
        let got = rec.samples();
        assert!(got.as_chunks::<2>().0.iter().all(|f| *f == [8192, -8192]), "{:?}", &got[..4]);
    }

    #[test]
    fn the_apps_volume_scales_the_copy() {
        let rec = Recorder::casting();
        Tap::new(sine(44100, 2, 0.5, 1000.0, 0.8), rec.clone(), Gain::new(0.25)).for_each(drop);
        assert!((rms(&rec.samples()) - 0.2 / 2f64.sqrt()).abs() < 0.005);
    }

    #[test]
    fn casting_mutes_this_computer_and_not_casting_copies_nothing() {
        let rec = Arc::new(Recorder::default());
        let mut tap = Tap::new(sine(44100, 2, 1.0, 1000.0, 0.5), rec.clone(), Gain::new(1.0));
        // Not casting: plays here, nothing copied.
        let here: Vec<f32> = tap.by_ref().take(8820).collect();
        assert!(here.iter().any(|&s| s.abs() > 0.4));
        assert!(rec.chunks.lock().unwrap().is_empty());
        // Casting starts mid-song: silence here from the next frame, the copy has the sound.
        rec.casting.store(true, Ordering::SeqCst);
        let muted: Vec<f32> = tap.by_ref().take(44100).collect();
        assert!(muted.iter().all(|&s| s == 0.0), "no sound out of this computer while casting");
        assert!(rms(&rec.samples()) > 0.3);
        // Casting ends: sound here again.
        rec.casting.store(false, Ordering::SeqCst);
        let back: Vec<f32> = tap.by_ref().take(8820).collect();
        assert!(back.iter().any(|&s| s.abs() > 0.4));
    }

    #[test]
    fn resampler_rate_change_and_reset() {
        let mut r = Resampler::default();
        let mut out = Vec::new();
        r.push([0.0, 0.0], 44100, 1.0, &mut out);
        assert!(out.is_empty(), "the first frame waits for the next");
        r.push([1.0, -1.0], 44100, 1.0, &mut out);
        assert_eq!(out, vec![0, 0]);
        out.clear();
        // Half the rate: two output frames per input frame, the middle one interpolated.
        r.push([0.0, 0.0], 22050, 1.0, &mut out);
        assert_eq!(out, vec![32767, -32767, 16384, -16384]);
        r.reset();
        out.clear();
        r.push([0.5, 0.5], 44100, 1.0, &mut out);
        assert!(out.is_empty());
    }
}
