// A diagnostic, not part of the app: how does the main process of castLabs Electron see its
// standard input on this system, and does a named pipe (a Unix socket elsewhere) work as the
// command channel instead? It starts a ten-line Electron app several ways, sends it lines and
// prints what arrived. No network, no window, no profile of the user's.
//
//   node packaging/windows/probe/stdin-probe.mjs <electron executable>
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';

const electron = process.argv[2];
const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'uzumaki-stdin-probe-'));
fs.writeFileSync(path.join(dir, 'package.json'), JSON.stringify({ name: 'stdin-probe', main: 'main.js' }));
fs.writeFileSync(
  path.join(dir, 'main.js'),
  `
const { app } = require('electron');
const fs = require('node:fs');
const net = require('node:net');
const out = process.env.PROBE_LOG;
const t0 = Date.now();
const log = (m) => fs.appendFileSync(out, (Date.now() - t0) + ' ms  ' + m + '\\n');
app.setPath('userData', process.env.PROBE_PROFILE);
app.disableHardwareAcceleration();
const describe = () => {
  try { log('fstat(0): ' + JSON.stringify((({ dev, mode, size }) => ({ dev, mode, size }))(fs.fstatSync(0)))); } catch (e) { log('fstat(0) throws: ' + e.message); }
  try { const s = process.stdin; log('process.stdin: ' + s.constructor.name + ' fd=' + s.fd + ' isTTY=' + s.isTTY + ' readable=' + s.readable); } catch (e) { log('process.stdin throws: ' + e.message); }
};
const listen = (what) => {
  log('listening on stdin (' + what + ')');
  try {
    process.stdin.on('data', (d) => log('stdin data ' + JSON.stringify(String(d))));
    process.stdin.on('end', () => log('stdin end'));
    process.stdin.on('close', () => log('stdin close'));
    process.stdin.on('error', (e) => log('stdin error ' + e.message));
  } catch (e) { log('listening throws: ' + e.message); }
};
log('started, platform ' + process.platform + ', electron ' + process.versions.electron);
describe();
if (process.env.PROBE_MODE === 'early') listen('before ready');
if (process.env.PROBE_PIPE) {
  const c = net.connect(process.env.PROBE_PIPE);
  c.on('connect', () => { log('pipe connected'); c.write('hello from electron\\n'); });
  c.on('data', (d) => log('pipe data ' + JSON.stringify(String(d))));
  c.on('end', () => log('pipe end'));
  c.on('close', () => log('pipe close'));
  c.on('error', (e) => log('pipe error ' + e.message));
}
app.whenReady().then(() => {
  log('app ready');
  process.stdout.write(JSON.stringify({ event: 'ready' }) + '\\n');
  if (process.env.PROBE_MODE === 'late') { describe(); listen('after ready'); }
  setTimeout(() => { log('done'); app.exit(0); }, 6000);
});
`,
);

function run(name, mode, extraEnv = {}, withPipe = false) {
  return new Promise((resolve) => {
    const logFile = path.join(dir, `${name}.log`);
    const profile = path.join(dir, `profile-${name}`);
    const pipeName = process.platform === 'win32' ? `\\\\.\\pipe\\uzumaki-probe-${process.pid}-${name}` : path.join(dir, `${name}.sock`);
    let server = null;
    const got = [];
    const start = () => {
      const env = { ...process.env, PROBE_LOG: logFile, PROBE_PROFILE: profile, PROBE_MODE: mode, ...extraEnv };
      if (withPipe) env.PROBE_PIPE = pipeName;
      const child = spawn(electron, [dir], { stdio: ['pipe', 'pipe', 'ignore'], env });
      let stdout = '';
      child.stdout.on('data', (d) => (stdout += d));
      child.stdin.on('error', (e) => got.push(`parent: writing to stdin failed: ${e.message}`));
      const timers = [1500, 3000, 4500].map((ms, i) => setTimeout(() => { try { child.stdin.write(`line ${i + 1}\n`); } catch (e) { got.push(`parent: write threw ${e.message}`); } }, ms));
      child.on('exit', (code) => {
        timers.forEach(clearTimeout);
        server?.close();
        console.log(`\n=== ${name}: electron exited with ${code}`);
        console.log(`stdout from electron: ${JSON.stringify(stdout)}`);
        for (const g of got) console.log(g);
        try { console.log(fs.readFileSync(logFile, 'utf8').trimEnd()); } catch { console.log('(no log written)'); }
        resolve();
      });
    };
    if (withPipe) {
      server = net.createServer((sock) => {
        got.push('parent: electron connected to the pipe');
        sock.on('data', (d) => got.push(`parent: pipe data ${JSON.stringify(String(d))}`));
        sock.on('error', () => {});
        [2000, 3500].forEach((ms, i) => setTimeout(() => { try { sock.write(`pipe line ${i + 1}\n`); } catch {} }, ms));
      });
      server.listen(pipeName, start);
    } else start();
  });
}

await run('early', 'early');
await run('late', 'late');
await run('late-no-attach-console', 'late', { ELECTRON_NO_ATTACH_CONSOLE: '1' });
await run('pipe', 'none', {}, true);
try { fs.rmSync(dir, { recursive: true, force: true, maxRetries: 5, retryDelay: 300 }); } catch {}
