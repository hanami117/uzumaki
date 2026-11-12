// Uzumaki playback bridge: a castLabs Electron process that loads music.apple.com
// (hidden unless --show) and drives the page's own MusicKit instance.
//
// Protocol: newline-delimited JSON.
//   stdin  <- commands  {"id": 1, "cmd": "search", "term": "..."}
//   stdout -> replies   {"id": 1, "ok": true, "result": ...} | {"id": 1, "ok": false, "error": "..."}
//          -> events    {"event": "ready" | "auth" | "storefront" | "nowPlaying" | "playbackState" | "time" | "widevine" | "window" | "error", ...}
// stdout is the protocol channel: never console.log here, use log() (stderr).
'use strict';

const { app, BrowserWindow, components, ipcMain, screen, session, shell } = require('electron');
const path = require('node:path');
const readline = require('node:readline');
const signin = require('./signin.js');

const START_URL = 'https://music.apple.com/';

// Login cookies / MusicKit tokens persist here. UZUMAKI_PROFILE_DIR (or the old CADENCE_PROFILE_DIR) points tests at a scratch
// profile so they never touch the user's login or saved volume.
app.setPath('userData', process.env.UZUMAKI_PROFILE_DIR || process.env.CADENCE_PROFILE_DIR || path.join(app.getPath('appData'), 'uzumaki', 'bridge'));

// Only one --disable-features switch takes effect, so keep all features in this call.
// No Chromium media session / MPRIS player: Rust owns the media keys.
// AudioServiceOutOfProcess off: audio runs in the browser process, no separate audio utility (about -15 MB PSS).
app.commandLine.appendSwitch('disable-features', 'MediaSessionService,HardwareMediaKeyHandling,AudioServiceOutOfProcess');
// No GPU compositing for a window nobody sees: gpu-process 106 -> 32 MB and renderer -60 MB (about -145 MB PSS together).
app.disableHardwareAcceleration();
// V8 favours memory over JIT speed in the renderer: renderer 272 -> 168 MB (about -100 MB PSS).
app.commandLine.appendSwitch('js-flags', '--optimize-for-size');
// Commands arrive over stdin, not from user gestures in the page.
app.commandLine.appendSwitch('autoplay-policy', 'no-user-gesture-required');
// Tests: silence Chromium's audio output without touching MusicKit's (persisted) volume.
if (process.env.UZUMAKI_MUTE === '1' || process.env.CADENCE_MUTE === '1') app.commandLine.appendSwitch('mute-audio');
// The player window is hidden; don't let Chromium deprioritise its renderer or timers.
app.commandLine.appendSwitch('disable-renderer-backgrounding');
app.commandLine.appendSwitch('disable-background-timer-throttling');
app.commandLine.appendSwitch('disable-backgrounding-occluded-windows');
// Present as plain Chrome (no "Electron/..." token) so Apple serves the normal web player.
app.userAgentFallback =
  `Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/${process.versions.chrome} Safari/537.36`;

const send = (obj) => {
  const t = Date.now();
  process.stdout.write(JSON.stringify(obj) + '\n');
  const ms = Date.now() - t;
  if (ms > 100) stalled(`stdout write blocked ${ms} ms (the app isn't reading)`, t);
};
const log = (...args) => console.error('[bridge]', ...args);

process.stdout.on('error', () => app.exit(0)); // parent went away (EPIPE)

let win = null;
let pageReady = false; // true once the page bridge found MusicKit.getInstance()
let authorized = null; // MusicKit's sign-in state from the last `ready`/`auth` event
let signInTried = false; // opened Apple's sign-in during this showing of the window
let sized = 'full'; // the window's size and title: 'full' (`show`) or 'signIn' (`signIn`)
let loadFailures = 0; // main-frame load failures in a row; back to 0 once the page is ready
let retryTimer = null;
let quitting = false;
const sentAt = new Map(); // setQueue id -> when it was sent, for the timing log

// ---- song-start timing log
// For the first song after launch, and any song that takes over 2.5 s to start, the log gets a
// timeline: everything from launch to the first click (the "boot" buffer), then from the click until
// audio moves: the page's requests (host and path only, never query strings), the page's own marks,
// MusicKit's states, replies, and main-process stalls. Times are relative to the first click.
const bootAt = Date.now();
let readyAt = 0;
const inflight = new Map(); // command id -> name, for "what else was in flight"
const reqs = new Map(); // request id -> { at, label }, every non-image request still running
const SLOW_START_MS = 2500;
const BOOT_MAX_LINES = 400;
let first = true; // no song has started since launch
let bootOpen = true; // recording launch-time lines until the first click (or 60 s)
let bootLines = []; // [at, text, label?, ms?]
let bootDropped = 0;
let trace = null; // the open start window, see openTrace()
setTimeout(() => (bootOpen = false), 60000).unref();

function note(text, at = Date.now(), label, ms) {
  if (trace) trace.lines.push([at, text, label, ms]);
  else if (bootOpen) {
    if (bootLines.length < BOOT_MAX_LINES) bootLines.push([at, text, label, ms]);
    else bootDropped++;
  }
}

// Main-process stalls (a busy main process delays every reply) and a blocked stdout (the app not reading).
function stalled(text, at) {
  log(text);
  note(text, at);
}
let lastTick = Date.now();
const stallTimer = setInterval(() => {
  const now = Date.now();
  const gap = now - lastTick;
  lastTick = now;
  // Over a minute is a system suspend, not a stall.
  if (gap > 600 && gap < 60000) stalled(`main process stalled ${gap - 100} ms`, now - (gap - 100));
  for (const [id, r] of reqs) if (now - r.at > 600000) reqs.delete(id); // requests that never finished
}, 100);
stallTimer.unref();

const clickWhat = (m) =>
  Array.isArray(m.songIds) && m.songIds.length ? String(m.songIds[0]).slice(0, 40)
    : m.album ? `album ${String(m.album).slice(0, 40)}` : m.playlist ? `playlist ${String(m.playlist).slice(0, 40)}`
    : m.station ? `station ${String(m.station).slice(0, 40)}` : '?';

function armTrace(t) {
  clearTimeout(t.timer);
  t.timer = setTimeout(() => {
    if (trace === t) endTrace('no audio 30 s after the last click');
  }, 30000);
  t.timer.unref?.();
}

// A click (setQueue that plays) opens a window; more clicks inside it update what we wait for.
function click(msg) {
  const now = Date.now();
  const what = clickWhat(msg);
  const wantId = Array.isArray(msg.songIds) && msg.songIds.length ? String(msg.songIds[0]) : null;
  if (trace) {
    note(`click again: setQueue #${msg.id} ${what}`, now);
    Object.assign(trace, { lastClickAt: now, wantId, wantCmdId: msg.id, lastReplied: false, sawWanted: false });
    trace.clicks++;
    trace.cmds.add(msg.id);
  } else {
    const counts = {};
    for (const c of inflight.values()) counts[c] = (counts[c] || 0) + 1;
    const flight = Object.entries(counts).map(([c, n]) => `${c}×${n}`).join(' ');
    // Launch-time lines only matter for a click soon after launch.
    const boot = first && now - bootAt < 120000;
    const lines = boot ? bootLines : [];
    if (boot && bootDropped) lines.unshift([bootAt, `(${bootDropped} boot lines dropped)`]);
    trace = {
      t0: now, lastClickAt: now, clicks: 1, wantId, wantCmdId: msg.id, lastReplied: false, sawWanted: false,
      lines, first, cmds: new Set([msg.id]), timer: null,
      context: `${now - bootAt} ms after launch, ${readyAt ? `${now - readyAt} ms after ready` : 'before ready'}, ` +
        `in flight: ${flight || 'nothing'}`,
    };
    bootOpen = false;
    bootLines = [];
    bootDropped = 0;
  }
  note(`setQueue #${msg.id} sent: ${what}`, now);
  armTrace(trace);
}

function endTrace(why) {
  const t = trace;
  if (!t) return;
  trace = null;
  clearTimeout(t.timer);
  const now = Date.now();
  const a = now - t.lastClickAt;
  const b = now - t.t0;
  if (t.first || a > SLOW_START_MS || b > SLOW_START_MS) {
    const rel = (at) => { const ms = at - t.t0; return ms >= 0 ? `+${ms}` : String(ms); };
    log(`song start: audio ${a} ms after the last click (${why})` +
      (t.clicks > 1 ? `, ${b} ms after the first of ${t.clicks} clicks` : '') +
      (t.first ? '; first song after launch' : '') + `; first click ${t.context}`);
    t.lines.sort((x, y) => x[0] - y[0]);
    // Runs of the same request (e.g. many media range GETs) collapse into one line.
    for (let i = 0; i < t.lines.length; ) {
      const [at, text, label] = t.lines[i];
      let j = i + 1;
      let max = t.lines[i][3] || 0;
      while (label && j < t.lines.length && t.lines[j][2] === label) max = Math.max(max, t.lines[j++][3] || 0);
      log(`  ${rel(at).padStart(7)} ms  ${text}` + (j - i > 1 ? ` (+${j - i - 1} more, up to ${max} ms)` : ''));
      i = j;
    }
    for (const r of reqs.values()) if (now - r.at <= 120000) log(`  pending: ${r.label} since ${rel(r.at)} ms`);
  }
  first = false;
}

const shortUrl = (u) => {
  try {
    const x = new URL(u);
    const p = x.pathname.length > 90 ? x.pathname.slice(0, 87) + '...' : x.pathname;
    return x.host + p;
  } catch {
    return '?';
  }
};

const isApple = (url) => {
  try {
    const host = new URL(url).hostname;
    return host === 'apple.com' || host.endsWith('.apple.com');
  } catch {
    return false;
  }
};

// While hidden, cancel image requests and stop the page rendering (html{display:none}); MusicKit keeps playing.
// Measured about -75 MB (images) and about -12 MB (CSS) renderer PSS. Both are undone (reload / removeInsertedCSS) on show.
let hideCssKey = null;
let imagesBlocked = false;
async function applyHiddenCss(hidden) {
  if (!win || win.isDestroyed()) return;
  const wc = win.webContents;
  if (hidden && !hideCssKey) hideCssKey = await wc.insertCSS('html{display:none !important}').catch(() => null);
  else if (!hidden && hideCssKey) {
    const k = hideCssKey;
    hideCssKey = null;
    await wc.removeInsertedCSS(k).catch(() => {});
  }
}

// The guided sign-in's CSS layer (signin.js) is on while the window shows for `signIn`. The
// preload marks the page's own elements around Apple's frame; nothing is sent back.
function sendSignInCss() {
  if (!win || win.isDestroyed()) return;
  const on = sized === 'signIn' && win.isVisible();
  win.webContents.send('cadence:signin-css', on ? { on, css: signin.SIGN_IN_CSS, hosts: signin.SIGN_IN_HOSTS } : { on: false });
}

// `reason` on a hide: 'closed' (the user closed the window) or 'hidden' (the app asked).
function setVisible(visible, reason = 'hidden') {
  applyHiddenCss(!visible);
  if (visible && imagesBlocked) {
    imagesBlocked = false;
    win.webContents.reload(); // the page lost its images while hidden
  }
  if (visible) {
    win.show();
    // The Dock icon is hidden on macOS (see whenReady), so the app isn't activated by showing.
    if (process.platform === 'darwin') app.focus({ steal: true });
    win.focus();
    maybeSignIn();
  } else {
    win.hide();
    signInTried = false;
  }
  sendSignInCss();
  send(signin.windowEvent(visible, reason));
}

// The screen to place the window on: the one under `near` (our window), else under the pointer
// (the user just clicked in our window), else the main one.
function workAreaFor(near) {
  try {
    if (near && [near.x, near.y, near.w, near.h].every(Number.isFinite) && near.w > 0 && near.h > 0) {
      return screen.getDisplayMatching({ x: Math.round(near.x), y: Math.round(near.y), width: Math.round(near.w), height: Math.round(near.h) }).workArea;
    }
    return screen.getDisplayNearestPoint(screen.getCursorScreenPoint()).workArea;
  } catch {
    return screen.getPrimaryDisplay().workArea;
  }
}

// `signIn`: the window, smaller and centred (on `near`, our window's frame, when the app sends
// it: macOS; else on the screen: Linux, where our Wayland window can't say where it is), then
// Apple's sign-in. Showing it again only brings it to the front.
function signIn(near) {
  if (!win || win.isDestroyed()) return;
  if (sized !== 'signIn' || !win.isVisible()) {
    sized = 'signIn';
    win.setBounds(signin.signInBounds(near, workAreaFor(near)));
  }
  win.setTitle(signin.SIGN_IN_TITLE);
  win.setBackgroundColor(signin.SIGN_IN_BG);
  setVisible(true);
}

// `show`: the full window, as before (debugging, `--login`, the app's banner).
function showFull() {
  if (sized !== 'full') {
    sized = 'full';
    win.setSize(signin.FULL_SIZE.width, signin.FULL_SIZE.height);
    win.center();
    win.setTitle(signin.FULL_TITLE);
  }
  setVisible(true);
}

// The window shows to sign in: open Apple's sign-in straight away (what the page's Sign In button
// does) instead of leaving the user on the store page. Once per showing, so closing Apple's dialog
// sticks; page reloads during the sign-in redirects don't start it again.
function maybeSignIn() {
  if (signInTried || !pageReady || authorized !== false || !win || win.isDestroyed() || !win.isVisible()) return;
  signInTried = true;
  log('signed out with the window showing: opening Apple sign-in');
  win.webContents
    .executeJavaScript(`window.__cadence ? window.__cadence.handle({cmd: 'authorize'}) : {ok: false, error: 'page bridge not loaded'}`)
    .then((r) => r?.ok || log('authorize failed:', r?.error))
    .catch((err) => log('authorize threw:', String(err?.message ?? err)));
}

function createWindow(show) {
  win = new BrowserWindow({
    width: 1100,
    height: 800,
    show,
    title: signin.FULL_TITLE,
    autoHideMenuBar: true,
    // Linux: a dialog window (_NET_WM_WINDOW_TYPE_DIALOG), so tiling window managers float it
    // instead of tiling the sign-in into half the screen. Electron sets the type only here.
    ...(process.platform === 'linux' ? { type: 'dialog' } : {}),
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      contextIsolation: true,
      sandbox: true,
      nodeIntegration: false,
      backgroundThrottling: false,
      autoplayPolicy: 'no-user-gesture-required',
    },
  });

  // Closing the login window only hides it; playback keeps going.
  win.on('close', (e) => {
    if (!quitting) {
      e.preventDefault();
      setVisible(false, 'closed');
    }
  });
  // Our title, not the page's ("Apple Music - Web Player").
  win.on('page-title-updated', (e) => e.preventDefault());

  // Apple sign-in may use popups; anything else opens in the system browser.
  win.webContents.setWindowOpenHandler(({ url }) => {
    if (isApple(url)) return { action: 'allow', overrideBrowserWindowOptions: { autoHideMenuBar: true } };
    shell.openExternal(url);
    return { action: 'deny' };
  });

  win.webContents.on('did-start-navigation', (details) => {
    if (details.isMainFrame && !details.isSameDocument) pageReady = false;
  });
  win.webContents.on('render-process-gone', (_e, d) => {
    pageReady = false;
    send({ event: 'error', message: `renderer gone: ${d.reason}` });
  });
  win.webContents.on('did-fail-load', (_e, code, desc, url, isMainFrame) => {
    // -3 (ERR_ABORTED) is a navigation replaced by another one, e.g. during sign-in redirects.
    if (!isMainFrame || code === -3) return;
    // A network blip at launch (waking from sleep, Wi-Fi still joining) used to leave the app
    // without a player until it was restarted. Load again after 2, 4, 8, 16 s, then every 30 s;
    // the app hears about it only when it isn't a blip.
    loadFailures++;
    log(`load failed (${code} ${desc}): ${url}; trying again (${loadFailures})`);
    if (loadFailures === 3) send({ event: 'error', message: `Can't reach Apple Music (${code} ${desc}). Still trying.` });
    clearTimeout(retryTimer);
    retryTimer = setTimeout(() => {
      if (!quitting && win && !win.isDestroyed()) win.loadURL(url).catch(() => {}); // did-fail-load reports it
    }, Math.min(30_000, 1000 * 2 ** loadFailures));
  });

  // Only the player window's image requests, only while hidden: sign-in popups and the visible window are untouched.
  session.defaultSession.webRequest.onBeforeRequest({ urls: ['*://*/*'] }, (details, cb) => {
    const block = win && !win.isDestroyed() && !win.isVisible() && details.webContentsId === win.webContents.id &&
      (details.resourceType === 'image' || (details.resourceType === 'media' && /mzstatic\.com\/image\//.test(details.url)));
    if (block) imagesBlocked = true;
    else if (details.resourceType !== 'image') reqs.set(details.id, { at: Date.now(), label: `${details.method} ${shortUrl(details.url)}` });
    cb({ cancel: !!block });
  });
  const reqDone = (d, what) => {
    const r = reqs.get(d.id);
    if (!r) return;
    reqs.delete(d.id);
    const ms = Date.now() - r.at;
    note(`${r.label} -> ${what} in ${ms} ms`, r.at, r.label, ms);
  };
  session.defaultSession.webRequest.onCompleted({ urls: ['*://*/*'] }, (d) => reqDone(d, d.statusCode));
  session.defaultSession.webRequest.onErrorOccurred({ urls: ['*://*/*'] }, (d) => reqDone(d, d.error));
  win.webContents.on('dom-ready', () => {
    hideCssKey = null; // a new document starts without our CSS
    applyHiddenCss(!win.isVisible());
    sendSignInCss(); // sign-in redirects load the page again
  });
  win.loadURL(START_URL);
}

// Warm the connections (DNS + TCP + TLS) to Apple's playback hosts, so the first song doesn't pay for
// them inside MusicKit's serial webPlayback -> cert -> licence chain. Idle sockets get closed, so it
// runs again before a play if the last warm-up is over 60 s old. No timer: no traffic while idle.
const PRECONNECT_HOSTS = ['play.music.apple.com', 'play.itunes.apple.com', 'aod-ssl.itunes.apple.com'];
const PRECONNECT_MAX_AGE_MS = 60 * 1000;
let preconnectedAt = 0;
function preconnect(why) {
  const now = Date.now();
  if (now - preconnectedAt < PRECONNECT_MAX_AGE_MS) return;
  preconnectedAt = now;
  try {
    for (const host of PRECONNECT_HOSTS) session.defaultSession.preconnect({ url: `https://${host}`, numSockets: 1 });
    log(`preconnect (${why}): ${PRECONNECT_HOSTS.join(', ')}`);
  } catch (err) {
    log(`preconnect failed: ${err?.message ?? err}`);
  }
}

// Everything the page bridge sends (replies + events) goes straight to stdout.
ipcMain.on('cadence:out', (e, msg) => {
  if (!win || e.sender !== win.webContents) return; // ignore sign-in popups
  if (msg && msg.event === 'log') return log(msg.message); // diagnostics, for the log only
  if (msg && msg.event === 'trace') return note(String(msg.text), Number.isFinite(msg.at) ? msg.at : Date.now()); // page marks: timeline only, never to the app
  if (msg && msg.event === 'ready') {
    pageReady = true;
    loadFailures = 0;
    clearTimeout(retryTimer);
    readyAt = Date.now();
    log(`ready ${readyAt - bootAt} ms after launch`);
    note(`ready ${readyAt - bootAt} ms after launch`, readyAt);
    preconnect('ready');
  }
  if (msg && (msg.event === 'ready' || msg.event === 'auth')) {
    authorized = !!msg.authorized;
    if (msg.event === 'ready') maybeSignIn(); // after the line above, so a `--show` start signs in too
  }
  if (msg && msg.id != null) inflight.delete(msg.id);
  const t = trace;
  if (t && msg) {
    if (msg.event === 'playbackState') note(`state ${msg.state}`);
    else if (msg.event === 'error') note(`error ${msg.message}`);
    else if (msg.event === 'nowPlaying' && msg.item) {
      note(`nowPlaying ${msg.item.id}`);
      if (t.wantId && (String(msg.item.id) === t.wantId || String(msg.item.catalogId) === t.wantId)) t.sawWanted = true;
    } else if (msg.id != null && t.cmds.has(msg.id)) {
      const r = msg.result || {};
      note(`#${msg.id} replied ` + (!msg.ok ? `failed: ${msg.error}` : r.superseded ? 'superseded' : 'ok'));
      if (msg.id === t.wantCmdId) {
        if (!msg.ok) endTrace('setQueue failed');
        else if (!r.superseded) t.lastReplied = true;
      }
    }
    // The old song's ticks keep coming until MusicKit switches: wait for the wanted one.
    if (trace === t && msg.event === 'time' && msg.current > 0 && (t.sawWanted || t.lastReplied)) endTrace('audio moving');
  }
  if (msg && sentAt.has(msg.id)) {
    const r = msg.result || {};
    log(`setQueue: ${Date.now() - sentAt.get(msg.id)} ms to reply` +
      (!msg.ok ? ` ERROR ${msg.error}` : r.superseded ? ' (superseded by a newer click)'
        : ` (waited ${r.waitMs} ms, queued ${r.queuedMs} ms, playing ${r.playMs} ms)`));
    sentAt.delete(msg.id);
  }
  send(msg);
});

// Silence at once (Chromium's shutdown takes a moment and the page would keep playing), then
// quit cleanly so cookies and storage are flushed; force it if that hangs.
function shutdown() {
  if (quitting) return;
  quitting = true;
  try {
    win?.webContents.setAudioMuted(true);
  } catch {}
  setTimeout(() => app.exit(0), 2000).unref();
  app.quit();
}

// Sign out: MusicKit forgets the user token, then the profile drops every apple.com cookie (the
// Apple ID session too, so signing in again asks for the password) and music.apple.com's storage,
// and the page reloads signed out (a new `ready` with authorized: false follows).
async function signOut() {
  if (pageReady) {
    try {
      const r = await win.webContents.executeJavaScript(
        `window.__cadence ? window.__cadence.handle({cmd: 'unauthorize'}) : {ok: false, error: 'page bridge not loaded'}`,
      );
      if (!r?.ok) log('signOut: unauthorize failed:', r?.error);
    } catch (err) {
      log('signOut: unauthorize threw:', String(err?.message ?? err));
    }
  }
  const ses = session.defaultSession;
  const cookies = await ses.cookies.get({});
  let removed = 0;
  for (const c of cookies) {
    const host = (c.domain || '').replace(/^\./, '');
    if (host !== 'apple.com' && !host.endsWith('.apple.com')) continue;
    await ses.cookies.remove(`https://${host}${c.path || '/'}`, c.name).catch(() => {});
    removed++;
  }
  await ses.clearStorageData({
    origin: 'https://music.apple.com',
    storages: ['localstorage', 'indexdb', 'serviceworkers', 'cachestorage'],
  });
  await ses.cookies.flushStore().catch(() => {});
  log(`signOut: removed ${removed} cookies, cleared music.apple.com storage`);
  pageReady = false;
  win.webContents.reload();
  return { cookiesRemoved: removed };
}

function handle(msg) {
  const id = msg.id ?? null;
  const reply = (ok, extra) => send({ id, ok, ...extra });
  switch (msg.cmd) {
    case 'signOut':
      signOut().then(
        (result) => reply(true, { result }),
        (err) => reply(false, { error: String(err?.message ?? err) }),
      );
      return;
    case 'show':
      showFull();
      return reply(true, { result: null });
    case 'signIn':
      signIn(msg.near);
      return reply(true, { result: null });
    case 'hide':
      setVisible(false);
      return reply(true, { result: null });
    case 'reload':
      win.webContents.reload();
      return reply(true, { result: null });
    case 'quit':
      reply(true, { result: null });
      return shutdown();
    default:
      if (!pageReady) return reply(false, { error: 'MusicKit not ready yet (page still loading)' });
      // `api` too: opening a playlist or album reads it first, so the sockets are warm by the click.
      if (msg.cmd === 'setQueue' || msg.cmd === 'play' || msg.cmd === 'api') preconnect(msg.cmd);
      if (msg.cmd === 'setQueue') {
        sentAt.set(id, Date.now());
        if (msg.startPlaying !== false) click(msg);
      }
      if (id != null) inflight.set(id, msg.cmd === 'api' ? `api ${String(msg.path || '').split('?')[0].replace(/\/[^/]*\d[^/]*/g, '/…')}` : msg.cmd);
      win.webContents.send('cadence:cmd', msg);
  }
}

app.whenReady().then(async () => {
  try {
    await components.whenReady(); // downloads the Widevine CDM on first run
    send({ event: 'widevine', ok: true, status: components.status() });
  } catch (err) {
    send({ event: 'widevine', ok: false, error: String(err?.message ?? err), status: components.status() });
  }

  // macOS: no second Dock icon for the hidden player; Uzumaki's own icon is the app. The sign-in
  // window still shows (setVisible activates it).
  if (process.platform === 'darwin') app.dock?.hide();
  createWindow(process.argv.includes('--show'));

  const rl = readline.createInterface({ input: process.stdin });
  rl.on('line', (line) => {
    line = line.trim();
    if (!line) return;
    let msg;
    try {
      msg = JSON.parse(line);
    } catch {
      return send({ event: 'error', message: `bad JSON on stdin: ${line.slice(0, 200)}` });
    }
    handle(msg);
  });
  rl.on('close', shutdown); // parent closed our stdin
});

app.on('before-quit', () => {
  quitting = true;
});
app.on('window-all-closed', () => app.quit());

log('started, userData =', app.getPath('userData'));
