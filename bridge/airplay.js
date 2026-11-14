// AirPlay spike (stage 1-2 of docs/research/airplay-sender.md), off unless UZUMAKI_AIRPLAY_SPIKE=1.
//
// Capture: the preload taps MusicKit's <audio> element with Web Audio (createMediaElementSource ->
// AudioWorklet). That is the player's own output, already decoded by Chromium; nothing here
// touches keys, the CDM or the encrypted stream. The worklet turns it into s16le stereo at 44.1 kHz,
// in 4096-frame chunks (~93 ms), and the preload hands the chunks to main.js over IPC.
// Transport: main.js writes the raw chunks to the Unix socket the app named in
// UZUMAKI_AIRPLAY_PCM_SOCKET (not stdin/stdout, which carry the JSON protocol). Nothing is written to
// disk. While the socket is connected (the app is casting) the local output is silenced with a gain
// node in the graph, never with MusicKit's volume (which MusicKit saves).
// The pure helpers below are shared with the worklet (as source text) and the node tests.
'use strict';

const net = require('node:net');

const SAMPLE_RATE = 44100;
const CHUNK_FRAMES = 4096;

const enabled = (env = process.env) => env.UZUMAKI_AIRPLAY_SPIKE === '1';
const socketPath = (env = process.env) => (enabled(env) && env.UZUMAKI_AIRPLAY_PCM_SOCKET) || null;

// Float [-1, 1] channels -> interleaved s16le, written into `out` (Int16Array) from frame `at`.
// One input channel is copied to both sides; clipping saturates.
function toS16(left, right, out, at) {
  const r = right || left;
  for (let i = 0; i < left.length; i++) {
    let a = left[i] * 32767;
    let b = r[i] * 32767;
    a = a > 32767 ? 32767 : a < -32768 ? -32768 : a;
    b = b > 32767 ? 32767 : b < -32768 ? -32768 : b;
    out[(at + i) * 2] = Math.round(a) | 0;
    out[(at + i) * 2 + 1] = Math.round(b) | 0;
  }
}

// Level of an interleaved s16 buffer: { rms, peak } in [0, 1] and the RMS in dBFS (-Infinity for silence).
function level(samples) {
  let sum = 0;
  let peak = 0;
  for (let i = 0; i < samples.length; i++) {
    const v = samples[i] / 32768;
    sum += v * v;
    const m = v < 0 ? -v : v;
    if (m > peak) peak = m;
  }
  const rms = samples.length ? Math.sqrt(sum / samples.length) : 0;
  return { rms, peak, db: rms > 0 ? 20 * Math.log10(rms) : -Infinity };
}

// The AudioWorklet module, as source text (the preload loads it from a blob URL). It runs on the
// audio thread: fill a chunk, post it with its buffer transferred, start the next.
function workletSource() {
  return `'use strict';
const CHUNK_FRAMES = ${CHUNK_FRAMES};
const toS16 = ${toS16.toString()};
class UzumakiTap extends AudioWorkletProcessor {
  constructor() {
    super();
    this.buf = new Int16Array(CHUNK_FRAMES * 2);
    this.fill = 0;
  }
  process(inputs) {
    const input = inputs[0];
    const frames = input && input[0] ? input[0].length : 128;
    // No input connected (or a silent graph): Chromium hands over no channels; that is silence.
    const left = input && input[0] ? input[0] : new Float32Array(frames);
    const right = input && input[1] ? input[1] : left;
    let done = 0;
    while (done < frames) {
      const n = Math.min(frames - done, CHUNK_FRAMES - this.fill);
      toS16(left.subarray(done, done + n), right.subarray(done, done + n), this.buf, this.fill);
      this.fill += n;
      done += n;
      if (this.fill === CHUNK_FRAMES) {
        this.port.postMessage(this.buf.buffer, [this.buf.buffer]);
        this.buf = new Int16Array(CHUNK_FRAMES * 2);
        this.fill = 0;
      }
    }
    return true;
  }
}
registerProcessor('uzumaki-tap', UzumakiTap);
`;
}

// The once-a-second capture log line (levels only, never audio).
function levelLine({ chunks, frames, rms, peak, sending, localMuted, ctxState, elements, elementVolume, path }) {
  const db = rms > 0 ? (20 * Math.log10(rms)).toFixed(1) : '-inf';
  return `airplay capture: rms ${rms.toFixed(5)} (${db} dBFS) peak ${peak.toFixed(4)}; ${chunks} chunks, ${frames} frames/s; ` +
    `${path}, context ${ctxState}, ${elements} element(s), element volume ${elementVolume}; ` +
    `${sending ? 'sending to the app' : 'not sending'}, local output ${localMuted ? 'muted' : 'on'}`;
}

// Main-process side of the PCM socket: connects to the app's socket, retrying every second while it
// isn't there (the app binds it only once the speaker accepted the session). `onState(connected)`
// tells main.js when casting starts and stops, to mute the local output. Chunks written while the
// socket is down, or while over `maxQueued` bytes are waiting, are dropped (live audio: late is useless).
class PcmLink {
  constructor(path, { log = () => {}, onState = () => {}, retryMs = 1000, maxQueued = 1 << 20 } = {}) {
    Object.assign(this, { path, log, onState, retryMs, maxQueued });
    this.sock = null;
    this.connected = false;
    this.closed = false;
    this.timer = null;
    this.dropped = 0;
    this.sent = 0;
    this.lastError = null;
    this.connect();
  }

  connect() {
    if (this.closed) return;
    const sock = net.createConnection(this.path);
    this.sock = sock;
    sock.on('connect', () => {
      this.connected = true;
      this.lastError = null;
      this.log(`airplay: connected to the app's PCM socket ${this.path}`);
      this.onState(true);
    });
    sock.on('error', (err) => {
      // ENOENT / ECONNREFUSED: the app isn't casting (yet). Logged once per kind, not every second.
      if (err.code !== this.lastError) this.log(`airplay: PCM socket ${this.path}: ${err.code || err.message}`);
      this.lastError = err.code;
    });
    sock.on('close', () => {
      const was = this.connected;
      this.connected = false;
      if (this.sock === sock) this.sock = null;
      if (was) {
        this.log(`airplay: PCM socket closed (${this.sent} bytes sent, ${this.dropped} chunks dropped)`);
        this.onState(false);
      }
      if (!this.closed) {
        clearTimeout(this.timer);
        this.timer = setTimeout(() => this.connect(), this.retryMs);
        this.timer.unref?.();
      }
    });
  }

  // `bytes`: a Uint8Array/Buffer of whole s16le stereo frames.
  write(bytes) {
    if (!this.connected || !this.sock) return false;
    if (this.sock.writableLength > this.maxQueued) {
      this.dropped++;
      return false;
    }
    this.sock.write(Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength));
    this.sent += bytes.byteLength;
    return true;
  }

  close() {
    this.closed = true;
    clearTimeout(this.timer);
    this.sock?.destroy();
  }
}

module.exports = { SAMPLE_RATE, CHUNK_FRAMES, enabled, socketPath, toS16, level, workletSource, levelLine, PcmLink };
