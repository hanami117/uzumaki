// node --test bridge/   (no Electron, no network, no audio device)
'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const net = require('node:net');
const os = require('node:os');
const path = require('node:path');
const vm = require('node:vm');
const airplay = require('./airplay.js');

test('the PCM socket comes from the environment the app sets', () => {
  assert.equal(airplay.socketPath({}), null);
  assert.equal(airplay.socketPath({ UZUMAKI_AIRPLAY_PCM_SOCKET: '/tmp/x.sock' }), '/tmp/x.sock');
});

test('nothing of AirPlay runs in the bridge until the app asks', () => {
  const main = fs.readFileSync(path.join(__dirname, 'main.js'), 'utf8');
  // airplay.js is loaded lazily, at the first `airplay` command, and nowhere else.
  assert.equal(main.match(/require\('\.\/airplay\.js'\)/g).length, 1);
  assert.match(main, /airplay \?\?= require\('\.\/airplay\.js'\)/);
  assert.match(main, /case 'airplay':\s+airplaySet\(msg\.on, msg\.socket\)/);
  // The app may name the socket (the Chromecast session's); otherwise the env's.
  assert.match(main, /const path = captureSocket \|\| airplay\.socketPath\(\);/);
  // The page gets its graph only on a capture request.
  const preload = fs.readFileSync(path.join(__dirname, 'preload.js'), 'utf8');
  assert.match(preload, /if \(!capture\) return report\(\); \/\/ never cast in this document/);
  assert.equal(preload.match(/new AudioContext\(/g).length, 1);
});

test('float channels become interleaved, saturated s16', () => {
  const out = new Int16Array(8);
  airplay.toS16(Float32Array.of(0, 1, -1, 2), Float32Array.of(0.5, -0.5, 0, -2), out, 0);
  assert.deepEqual([...out], [0, 16384, 32767, -16383, -32767, 0, 32767, -32768]);
  const mono = new Int16Array(4);
  airplay.toS16(Float32Array.of(0.25, -0.25), null, mono, 0);
  assert.deepEqual([...mono], [8192, 8192, -8192, -8192]);
});

test('level reports RMS and peak, silence is -inf dBFS', () => {
  assert.equal(airplay.level(new Int16Array(16)).db, -Infinity);
  const l = airplay.level(Int16Array.of(16384, -16384, 16384, -16384));
  assert.ok(Math.abs(l.rms - 0.5) < 1e-9);
  assert.ok(Math.abs(l.peak - 0.5) < 1e-9);
  assert.ok(Math.abs(l.db - -6.0206) < 1e-3);
});

test('the worklet posts 4096-frame chunks of s16le stereo', () => {
  const posted = [];
  let Processor = null;
  const ctx = {
    AudioWorkletProcessor: class { constructor() { this.port = { postMessage: (b, t) => posted.push({ b, t }) }; } },
    registerProcessor: (name, cls) => { assert.equal(name, 'uzumaki-tap'); Processor = cls; },
    Float32Array, Int16Array, Math,
  };
  vm.runInNewContext(airplay.workletSource(), ctx);
  const p = new Processor();
  const left = new Float32Array(128).fill(0.5);
  const right = new Float32Array(128).fill(-0.5);
  for (let i = 0; i < 32; i++) assert.equal(p.process([[left, right]]), true); // 32 × 128 = 4096 frames
  assert.equal(posted.length, 1);
  const s = new Int16Array(posted[0].b);
  assert.equal(s.length, airplay.CHUNK_FRAMES * 2);
  assert.equal(s[0], 16384);
  assert.equal(s[1], -16383);
  assert.equal(posted[0].t[0], posted[0].b, 'the buffer is transferred, not copied');
  p.process([[]]); // no input channels: silence, no throw
});

test('the PCM link connects when the app binds, writes, and reports the state', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'uzu-airplay-'));
  const sock = path.join(dir, 'pcm.sock');
  const states = [];
  const link = new airplay.PcmLink(sock, { retryMs: 50, onState: (s) => states.push(s) });
  try {
    await new Promise((r) => setTimeout(r, 120)); // not there yet: retries quietly
    assert.equal(link.connected, false);
    assert.equal(link.write(new Uint8Array(4)), false);
    const got = [];
    const conns = [];
    const server = net.createServer((c) => { conns.push(c); c.on('data', (d) => got.push(d)); });
    await new Promise((r) => server.listen(sock, r));
    for (let i = 0; i < 40 && !link.connected; i++) await new Promise((r) => setTimeout(r, 25));
    assert.equal(link.connected, true);
    assert.equal(link.write(Uint8Array.of(1, 2, 3, 4)), true);
    for (let i = 0; i < 40 && Buffer.concat(got).length < 4; i++) await new Promise((r) => setTimeout(r, 25));
    assert.deepEqual([...Buffer.concat(got)], [1, 2, 3, 4]);
    // The app stops casting: the link reports it and the page gets its local output back.
    const closed = new Promise((r) => server.close(r));
    for (const c of conns) c.destroy();
    await closed;
    for (let i = 0; i < 40 && states.length < 2; i++) await new Promise((r) => setTimeout(r, 25));
    assert.deepEqual(states, [true, false]);
  } finally {
    link.close();
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

// ---- the page's tap (createTap in preload.js), against a fake Web Audio graph and DOM.
function loadCreateTap() {
  const src = fs.readFileSync(path.join(__dirname, 'preload.js'), 'utf8');
  const m = src.match(/\/\/ ---- createTap:begin\n([\s\S]*?)\/\/ ---- createTap:end/);
  assert.ok(m, 'createTap markers in preload.js');
  return vm.runInNewContext(`${m[1]}; createTap`, { Promise, Int16Array, Math, String, Number, URL, WeakSet, Error });
}

function fakeAudio({ state = 'running', workletDelay = 20 } = {}) {
  const log = [];
  const listeners = [];
  class Node {
    constructor(name) {
      this.name = name;
      this.outs = new Set();
      this.gain = { value: 1, target: 1, setTargetAtTime: (v) => { this.gain.target = v; } };
    }
    connect(n) {
      if (!n || !(n instanceof Node)) throw new TypeError("Failed to execute 'connect' on 'AudioNode': Overload resolution failed.");
      this.outs.add(n);
      return n;
    }
    disconnect(n) {
      if (n && !this.outs.has(n)) throw new Error('the given destination is not connected');
      n ? this.outs.delete(n) : this.outs.clear();
    }
  }
  const ctx = {
    state, sampleRate: 44100, currentTime: 0,
    destination: new Node('destination'),
    createGain: () => new Node('gain'),
    createScriptProcessor: () => new Node('script'),
    createMediaElementSource(el) {
      if (el.broken) throw new Error('HTMLMediaElement already connected previously to a different MediaElementSourceNode');
      const n = new Node(`source ${el.id}`);
      el.source = n;
      return n;
    },
    resume() {
      ctx.resumes = (ctx.resumes || 0) + 1;
      return Promise.resolve();
    },
    addEventListener: (type, fn) => listeners.push(fn),
    setState(s) {
      ctx.state = s;
      listeners.forEach((fn) => fn());
    },
  };
  const doc = { audio: [], watchers: new Set(), timers: new Set() };
  const statuses = [];
  const tainted = [];
  const io = {
    newContext: () => ctx,
    loadWorklet: () => new Promise((r) => setTimeout(r, workletDelay)),
    newWorklet: () => Object.assign(new Node('worklet'), { port: {} }),
    elements: () => doc.audio,
    player: () => doc.audio[0] || null,
    watch: (fn) => { doc.watchers.add(fn); return () => doc.watchers.delete(fn); },
    every: (fn) => { doc.timers.add(fn); return () => doc.timers.delete(fn); },
    send: () => {},
    note: (n) => log.push(n),
    status: (st) => statuses.push(st),
    meter: () => {},
    origin: () => 'https://music.apple.com',
    tainted: (host) => tainted.push(host),
  };
  const add = (id, extra = {}) => {
    const el = { id, src: '', paused: true, volume: 1, ...extra };
    doc.audio.push(el);
    doc.watchers.forEach((fn) => fn());
    return el;
  };
  const tick = () => doc.timers.forEach((fn) => fn());
  // Does sound from this element reach the tap (worklet) and the speakers through the local gain?
  const reaches = (el, name) => {
    const seen = new Set();
    const walk = (n) => n && !seen.has(n) && (seen.add(n), n.name === name || [...n.outs].some(walk));
    return walk(el.source);
  };
  return { ctx, io, doc, add, tick, log, statuses, tainted, reaches, last: () => statuses[statuses.length - 1] };
}

const START = { source: '', capture: true, localMuted: false };
const MUTED = { source: '', capture: true, localMuted: true };

test('two messages in a row at the start of a cast: the second waits for the graph (the first-try bug)', async () => {
  const createTap = loadCreateTap();
  const f = fakeAudio({ workletDelay: 30 });
  const player = f.add('apple-music-player'); // a restored song, primed paused
  const tap = createTap(f.io);
  // main sends "capture" and, 1 ms later, "local output muted" when the socket connects.
  tap.set(START);
  await tap.set(MUTED);
  assert.ok(!f.log.some((l) => /could not connect/.test(l)), f.log.join('\n'));
  assert.ok(f.reaches(player, 'worklet'), 'the element feeds the tap');
  assert.equal(tap.state.count, 1);
  assert.deepEqual({ ...f.last() }, { ready: true, error: null, elements: 1 });
});

test('an <audio> created later, or a replacement, is tapped as soon as it is added', async () => {
  const createTap = loadCreateTap();
  const f = fakeAudio();
  const tap = createTap(f.io);
  await tap.set(MUTED);
  // Before the first play there is no element: nothing to capture yet, which is fine.
  assert.equal(f.last().ready, true);
  const first = f.add('apple-music-player');
  assert.ok(f.reaches(first, 'worklet') && f.reaches(first, 'destination'));
  // MusicKit replaces its element (a new song after the restored one).
  const second = f.add('apple-music-player-2');
  assert.ok(f.reaches(second, 'worklet'), 'the replacement feeds the tap too');
  assert.equal(tap.state.count, 2);
  assert.equal(f.last().ready, true);
});

test('an element the observer missed is tapped by the timer', async () => {
  const createTap = loadCreateTap();
  const f = fakeAudio();
  const tap = createTap(f.io);
  await tap.set(MUTED);
  const el = { id: 'quiet', volume: 1 };
  f.doc.audio.push(el); // added without a mutation record
  f.tick();
  assert.ok(f.reaches(el, 'worklet'));
});

test('not ready while the context is suspended or an element cannot be tapped', async () => {
  const createTap = loadCreateTap();
  const f = fakeAudio({ state: 'suspended' });
  f.add('apple-music-player');
  const tap = createTap(f.io);
  await tap.set(MUTED);
  assert.equal(f.last().ready, false);
  assert.match(f.last().error, /suspended/);
  assert.ok(f.ctx.resumes >= 1, 'asked to resume');
  f.ctx.setState('running');
  assert.equal(f.last().ready, true, 'ready once it runs');
  f.add('broken', { broken: true });
  assert.equal(f.last().ready, false);
  assert.match(f.last().error, /could not tap <audio id=broken>/);
});

test('capture off and on again: the tap reconnects, elements tapped meanwhile are found, the local gain follows', async () => {
  const createTap = loadCreateTap();
  const f = fakeAudio();
  const a = f.add('a');
  const tap = createTap(f.io);
  await tap.set(MUTED);
  assert.equal(f.ctx.destination.name, 'destination');
  await tap.set({ source: '', capture: false });
  assert.equal(tap.state.linked, false);
  assert.ok(!f.reaches(a, 'worklet'), 'tap branch disconnected');
  assert.ok(f.reaches(a, 'destination'), 'still heard here');
  assert.equal(f.last().ready, false);
  assert.ok(!f.log.some((l) => /could not/.test(l)), f.log.join('\n'));
  const b = f.add('b'); // while not capturing: not tapped
  assert.equal(b.source, undefined);
  await tap.set(START);
  await tap.set(MUTED);
  assert.ok(f.reaches(a, 'worklet') && f.reaches(b, 'worklet'));
  assert.equal(f.last().ready, true);
});

test('a page that never cast builds nothing when capture is off', async () => {
  const createTap = loadCreateTap();
  const f = fakeAudio();
  let built = 0;
  const tap = createTap({ ...f.io, newContext: () => (built++, f.ctx) });
  await tap.set({ source: '', capture: false });
  assert.equal(built, 0);
  assert.equal(tap.state, null);
});

test('an uploaded song in a second <audio>: the playing element is the one watched, and a taint is reported once', async () => {
  const createTap = loadCreateTap();
  const f = fakeAudio();
  // A catalog song made MusicKit's first player (MSE, now paused); the upload plays in a new one.
  const old = f.add('apple-music-player', { src: 'blob:https://music.apple.com/1', mediaKeys: {} });
  const tap = createTap(f.io);
  await tap.set(MUTED);
  const upload = f.add('apple-music-player', { src: 'https://store-032.blobstore.apple.com/x/audio', paused: false });
  assert.ok(f.reaches(upload, 'worklet'));
  f.tick(); // the scan and the meter
  assert.ok(f.log.some((l) => /element source: store-032\.blobstore\.apple\.com, crossOrigin unset, mediaKeys no, playing/.test(l)), f.log.join('\n'));
  assert.deepEqual(f.tainted, ['store-032.blobstore.apple.com']);
  f.tick();
  upload.paused = true;
  f.tick();
  upload.paused = false;
  f.tick();
  assert.equal(f.tainted.length, 1, 'once per file, a pause and play included');
  // Loaded in CORS mode (the page hook did its job): nothing to report.
  const next = f.add('apple-music-player', { src: 'https://store-032.blobstore.apple.com/y/audio', crossOrigin: 'use-credentials', paused: false });
  upload.paused = true;
  f.tick();
  assert.equal(f.tainted.length, 1);
  assert.ok(f.log.some((l) => /crossOrigin use-credentials/.test(l)));
  // MusicKit's own same-origin stream is never a taint.
  next.paused = true;
  old.paused = false;
  f.tick();
  assert.equal(f.tainted.length, 1);
});

// ---- uploadCorsHook (preload.js, page world) against a fake window.
function loadUploadHook() {
  const src = fs.readFileSync(path.join(__dirname, 'preload.js'), 'utf8');
  const m = src.match(/\/\/ ---- uploadCorsHook:begin\n([\s\S]*?)\/\/ ---- uploadCorsHook:end/);
  assert.ok(m, 'uploadCorsHook markers in preload.js');
  return vm.runInNewContext(`${m[1]}; uploadCorsHook`, { Object, String, WeakSet });
}

function fakeWindow() {
  const loads = [];
  const logs = [];
  let errorListener = null;
  class Element {
    constructor() { this.attrs = {}; }
    setAttribute(n, v) { this.attrs[n.toLowerCase()] = String(v); if (n.toLowerCase() === 'src') this.load(String(v)); }
    hasAttribute(n) { return n in this.attrs; }
    removeAttribute(n) { delete this.attrs[n]; }
    get crossOrigin() { return this.attrs.crossorigin ?? null; }
  }
  class HTMLMediaElement extends Element {
    constructor() { super(); this.currentTime = 0; this.paused = true; this.played = 0; }
    load(url) { loads.push({ url, cors: this.attrs.crossorigin ?? null }); this.currentSrc = url; }
    play() { this.played++; this.paused = false; return Promise.resolve(); }
  }
  Object.defineProperty(HTMLMediaElement.prototype, 'src', {
    configurable: true, enumerable: true,
    get() { return this.currentSrc || ''; },
    set(v) { this.load(String(v)); },
  });
  const w = {
    HTMLMediaElement, Element,
    __cadenceNative: { emit: (m) => logs.push(m.message) },
    addEventListener: (type, fn, capture) => { if (type === 'error' && capture) errorListener = fn; },
  };
  // An error event at `el`: the window's capture listener first, then the element's own (MusicKit).
  const fail = (el, code = 4) => {
    el.error = { code };
    let stopped = false;
    errorListener({ target: el, stopImmediatePropagation: () => { stopped = true; } });
    return stopped ? 'stopped' : 'reached MusicKit';
  };
  return { w, loads, logs, fail };
}

test("an uploaded file loads in CORS mode; MusicKit's own stream and other hosts are untouched", () => {
  const hook = loadUploadHook();
  const { w, loads } = fakeWindow();
  hook(w);
  hook(w); // twice: wraps once
  const el = new w.HTMLMediaElement();
  el.src = 'https://store-032.blobstore.apple.com/sq/audio?sig=1';
  el.src = 'blob:https://music.apple.com/abc';
  const other = new w.HTMLMediaElement();
  other.src = 'https://audio-ssl.itunes.apple.com/preview.m4a';
  const attr = new w.HTMLMediaElement();
  attr.setAttribute('src', 'https://store-001.blobstore.apple.com/a');
  assert.deepEqual(loads.map((l) => l.cors), ['use-credentials', 'use-credentials', null, 'use-credentials']);
  assert.equal(w.HTMLMediaElement.prototype.hasOwnProperty('src'), true);
});

test('when the CORS load fails, it loads again the old way at the same place, and MusicKit never sees the error', () => {
  const hook = loadUploadHook();
  const { w, loads, logs, fail } = fakeWindow();
  hook(w);
  const el = new w.HTMLMediaElement();
  el.src = 'https://store-032.blobstore.apple.com/sq/audio';
  el.currentTime = 12.5;
  assert.equal(fail(el), 'stopped');
  assert.deepEqual(loads.map((l) => l.cors), ['use-credentials', null]);
  assert.equal(loads[1].url, 'https://store-032.blobstore.apple.com/sq/audio');
  assert.equal(el.currentTime, 12.5);
  assert.equal(el.played, 1);
  assert.match(logs.join('\n'), /loading it in CORS mode failed/);
  // That element never asks for CORS again, and its later errors are MusicKit's.
  el.src = 'https://store-032.blobstore.apple.com/next/audio';
  assert.equal(loads[2].cors, null);
  assert.equal(fail(el), 'reached MusicKit');
  // An error on an element we didn't change is MusicKit's too.
  const mine = new w.HTMLMediaElement();
  mine.src = 'blob:https://music.apple.com/x';
  assert.equal(fail(mine), 'reached MusicKit');
});

test("main answers CORS for upload files: the page origin with credentials, the server's own CORS headers replaced", () => {
  const src = fs.readFileSync(path.join(__dirname, 'main.js'), 'utf8');
  const m = src.match(/\/\/ ---- uploadCors:begin\n([\s\S]*?)\/\/ ---- uploadCors:end/);
  assert.ok(m, 'uploadCors markers in main.js');
  const withCors = vm.runInNewContext(`${m[1]}; withCors`, { Object });
  const out = withCors({ 'Content-Range': ['bytes 0-1/2'], 'access-control-allow-origin': ['*'] }, 'https://music.apple.com');
  assert.deepEqual(JSON.parse(JSON.stringify(out)), {
    'Content-Range': ['bytes 0-1/2'],
    'Access-Control-Allow-Origin': ['https://music.apple.com'],
    'Access-Control-Allow-Credentials': ['true'],
  });
  // Only the player window's media requests to the upload host.
  assert.match(src, /onHeadersReceived\(\{ urls: \['https:\/\/\*\.blobstore\.apple\.com\/\*'\] \}/);
  assert.match(src, /d\.webContentsId === win\.webContents\.id && d\.resourceType === 'media'/);
  // The hook runs in the page's world before the page bridge (and MusicKit).
  const preload = fs.readFileSync(path.join(__dirname, 'preload.js'), 'utf8');
  assert.ok(preload.indexOf('(${uploadCorsHook})(window)') < preload.indexOf('(${pageBridge})()'));
});
