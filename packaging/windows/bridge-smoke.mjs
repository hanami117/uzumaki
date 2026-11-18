// Starts the playback bridge (castLabs Electron + bridge/main.js) the way the app does and checks
// that it answers: the Widevine component installs, music.apple.com's MusicKit is found, the page
// can get a Widevine key system, and a catalog search comes back. Signed out, muted, on a scratch
// profile: no account, no sound, nothing of the user's is touched. It plays nothing, so it does
// NOT prove that Apple grants a licence (that needs a signed-in person; see README-WINDOWS.txt).
//
//   node packaging/windows/bridge-smoke.mjs <bridge dir> [<electron executable>]
//
// Exit code 0: all checks passed. Used by .github/workflows/windows.yml; works on any system.
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import readline from 'node:readline';

const bridge = path.resolve(process.argv[2] || 'bridge');
const dist = path.join(bridge, 'node_modules', 'electron', 'dist');
const electron =
  process.argv[3] ||
  (process.platform === 'win32'
    ? path.join(dist, 'electron.exe')
    : process.platform === 'darwin'
      ? path.join(dist, 'Electron.app', 'Contents', 'MacOS', 'Electron')
      : path.join(dist, 'electron'));
if (!fs.existsSync(electron)) {
  console.error(`no Electron at ${electron}`);
  process.exit(2);
}

const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'uzumaki-smoke-'));
const logPath = path.join(profile, 'bridge.log');
const log = fs.openSync(logPath, 'w');
const args = [bridge];
// A CI container has no user namespaces for Chromium's sandbox (Linux only).
if (process.platform === 'linux' && process.env.CI) args.push('--no-sandbox');
const child = spawn(electron, args, {
  stdio: ['pipe', 'pipe', log],
  env: { ...process.env, UZUMAKI_PROFILE_DIR: profile, UZUMAKI_MUTE: '1' },
});

const TIMEOUT_MS = 180000;
const waiting = new Map(); // id -> resolve
const events = [];
const eventWaiters = [];
let nextId = 1;
let exited = false;

readline.createInterface({ input: child.stdout }).on('line', (line) => {
  let msg;
  try {
    msg = JSON.parse(line);
  } catch {
    return;
  }
  if (msg.event) {
    if (msg.event !== 'time') console.log('event', JSON.stringify(msg).slice(0, 400));
    events.push(msg);
    for (const w of [...eventWaiters]) w();
  } else if (msg.id != null && waiting.has(msg.id)) {
    waiting.get(msg.id)(msg);
    waiting.delete(msg.id);
  }
});
child.on('exit', (code) => {
  exited = true;
  console.log(`electron exited with ${code}`);
  for (const w of [...eventWaiters]) w();
});

const timeout = (ms, what) => new Promise((_, reject) => setTimeout(() => reject(new Error(`timed out waiting for ${what}`)), ms).unref());

function event(name) {
  const found = () => events.find((e) => e.event === name);
  return Promise.race([
    new Promise((resolve, reject) => {
      const check = () => {
        const e = found();
        if (e) resolve(e);
        else if (exited) reject(new Error(`electron ended before "${name}"`));
        else return false;
        const i = eventWaiters.indexOf(check);
        if (i >= 0) eventWaiters.splice(i, 1);
        return true;
      };
      if (!check()) eventWaiters.push(check);
    }),
    timeout(TIMEOUT_MS, `the "${name}" event`),
  ]);
}

function call(cmd) {
  const id = nextId++;
  const reply = new Promise((resolve) => waiting.set(id, resolve));
  child.stdin.write(JSON.stringify({ ...cmd, id }) + '\n');
  return Promise.race([reply, timeout(60000, `the reply to ${cmd.cmd}`)]);
}

let failed = 0;
const check = (ok, text) => {
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${text}`);
  if (!ok) failed++;
};

try {
  const wv = await event('widevine');
  check(wv.ok === true, `Widevine component ready: ${JSON.stringify(wv.status ?? wv.error)}`);
  const ready = await event('ready');
  check(true, `MusicKit found on music.apple.com (signed in: ${ready.authorized}, storefront: ${ready.storefront})`);
  const key = await call({ cmd: 'widevine' });
  check(key.ok === true && key.result?.available === true, `the page gets a Widevine key system: ${JSON.stringify(key.result ?? key.error).slice(0, 300)}`);
  const status = await call({ cmd: 'status' });
  check(status.ok === true, `status answers: ${JSON.stringify(status.result ?? status.error).slice(0, 200)}`);
  const search = await call({ cmd: 'search', term: 'bach cello suite', limit: 3 });
  const songs = search.result?.songs ?? [];
  check(search.ok === true && songs.length > 0, `catalog search answers (${songs.length} songs): ${JSON.stringify(search.result ?? search.error).slice(0, 200)}`);
} catch (e) {
  check(false, String(e.message ?? e));
}

// Asked to quit, the bridge ends by itself; closing its input must end it too (the app's way).
try {
  child.stdin.write(JSON.stringify({ cmd: 'quit', id: nextId++ }) + '\n');
  child.stdin.end();
} catch {}
await Promise.race([new Promise((r) => (exited ? r() : child.on('exit', r))), new Promise((r) => setTimeout(r, 15000))]);
check(exited, 'the bridge ends when told to quit');
if (!exited) child.kill();

console.log('--- bridge log (last lines)');
try {
  fs.closeSync(log);
  console.log(fs.readFileSync(logPath, 'utf8').split('\n').slice(-40).join('\n'));
} catch {}
try {
  fs.rmSync(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 500 });
} catch {}
process.exit(failed ? 1 : 0);
