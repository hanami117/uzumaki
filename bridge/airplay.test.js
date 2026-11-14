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
  assert.match(main, /case 'airplay':\s+airplaySet\(msg\.on\)/);
  // The page gets its graph only on a capture request.
  const preload = fs.readFileSync(path.join(__dirname, 'preload.js'), 'utf8');
  assert.match(preload, /if \(!capture\) return; \/\/ never cast in this document/);
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
