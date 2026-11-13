// Preload for the music.apple.com window. It runs in an isolated world, so it
// injects pageBridge() into the page's own JS world (where window.MusicKit lives)
// and relays messages: main -> page (commands) and page -> main (replies, events).
'use strict';

const { contextBridge, ipcRenderer, webFrame } = require('electron');

// Page -> main. Only this one function is visible to the page.
contextBridge.exposeInMainWorld('__cadenceNative', {
  emit: (msg) => ipcRenderer.send('cadence:out', msg),
});

// Main -> page. The page bridge always resolves to {ok, result} or {ok: false, error}.
ipcRenderer.on('cadence:cmd', async (_e, msg) => {
  let reply;
  try {
    reply = await webFrame.executeJavaScript(
      `window.__cadence ? window.__cadence.handle(${JSON.stringify(msg)}) : {ok: false, error: 'page bridge not loaded'}`,
    );
  } catch (err) {
    reply = { ok: false, error: String(err?.message ?? err) };
  }
  ipcRenderer.send('cadence:out', { id: msg.id ?? null, ...reply });
});

webFrame.executeJavaScript(`(${pageBridge})();`);

// ---- the sign-in window's CSS layer (bridge/signin.js)
// While the window shows for `signIn`, main sends the CSS and the sign-in hosts. This marks
// music.apple.com's own elements around Apple's sign-in frame (the dialog's box and its
// ancestors), so the CSS can fade the rest of the page into our background. It never touches the
// frame or anything inside the dialog, reads nothing from them and sends nothing back. Fails open:
// no frame, or no dialog box around it, and nothing is marked. This preload runs in the main frame
// only. To remove the layer, delete this block and SIGN_IN_CSS in signin.js.
let signin = null; // { style, observer, hosts, queued }
ipcRenderer.on('cadence:signin-css', (_e, msg) => {
  try {
    if (msg && msg.on) signinOn(String(msg.css || ''), Array.isArray(msg.hosts) ? msg.hosts.map(String) : []);
    else signinOff();
  } catch {
    signinOff();
  }
});

function signinOn(css, hosts) {
  signinOff();
  const style = document.createElement('style');
  style.id = 'uzumaki-signin';
  style.textContent = css;
  (document.head || document.documentElement).appendChild(style);
  const observer = new MutationObserver(() => signinQueue());
  observer.observe(document.documentElement, { childList: true, subtree: true, attributes: true, attributeFilter: ['src'] });
  signin = { style, observer, hosts, queued: false };
  signinMark();
}

function signinOff() {
  if (!signin) return;
  signin.observer.disconnect();
  signin.style.remove();
  signin = null;
  signinUnmark();
}

// Page changes come in bursts: look once per frame at most.
function signinQueue() {
  if (!signin || signin.queued) return;
  signin.queued = true;
  requestAnimationFrame(() => {
    if (!signin) return;
    signin.queued = false;
    signinMark();
  });
}

function signinUnmark() {
  document.documentElement.removeAttribute('data-uzu-signin');
  for (const el of document.querySelectorAll('[data-uzu-path], [data-uzu-dialog]')) {
    el.removeAttribute('data-uzu-path');
    el.removeAttribute('data-uzu-dialog');
  }
}

function signinMark() {
  const hosts = signin ? signin.hosts : [];
  let frame = null;
  for (const f of document.querySelectorAll('iframe')) {
    let host = '';
    try {
      host = new URL(f.src, location.href).hostname;
    } catch {}
    if (hosts.some((h) => host === h || host.endsWith('.' + h))) {
      frame = f;
      break;
    }
  }
  const box = frame && frame.parentElement && frame.parentElement.closest('dialog, [role="dialog"], [aria-modal="true"]');
  if (!box || box === document.body || box === document.documentElement) return signinUnmark();
  if (box.hasAttribute('data-uzu-dialog') && document.documentElement.hasAttribute('data-uzu-signin')) return;
  signinUnmark();
  box.setAttribute('data-uzu-dialog', '');
  for (let p = box.parentElement; p; p = p.parentElement) p.setAttribute('data-uzu-path', '');
  document.documentElement.setAttribute('data-uzu-signin', '');
}

// ---------------------------------------------------------------------------
// Everything below runs in the page's main world (stringified and injected).
function pageBridge() {
  if (window.__cadence) return;
  const emit = (msg) => window.__cadenceNative.emit(msg);
  // Timeline marks for the bridge's song-start log (main decides whether to keep them; never forwarded to the app).
  const mark = (text, at = Date.now()) => emit({ event: 'trace', at, text: String(text).slice(0, 120) });
  let mk = null;

  // Long tasks on the page's main thread explain stalls (a hung play() often is one).
  try {
    new PerformanceObserver((list) => {
      for (const e of list.getEntries()) {
        if (e.duration >= 200) mark(`page busy ${Math.round(e.duration)} ms`, Math.round(performance.timeOrigin + e.startTime));
      }
    }).observe({ type: 'longtask', buffered: true });
  } catch {}

  const stateName = (s) => {
    const states = (window.MusicKit && MusicKit.PlaybackStates) || {};
    for (const k of Object.keys(states)) if (states[k] === s && isNaN(Number(k))) return k;
    return String(s);
  };

  const artwork = (url, size = 600) => (url ? url.replace('{w}', size).replace('{h}', size).replace('{f}', 'jpg') : null);

  const itemInfo = (it) => {
    if (!it) return null;
    const a = it.attributes || {};
    return {
      id: it.id,
      title: it.title ?? a.name ?? null,
      artist: it.artistName ?? a.artistName ?? null,
      album: it.albumName ?? a.albumName ?? null,
      duration: a.durationInMillis != null ? a.durationInMillis / 1000 : null,
      artwork: a.artwork?.url ?? it.artworkURL ?? null, // template: the app picks the size
      artworkBg: a.artwork?.bgColor ?? null, // hex, catalog items only (Now Playing tint)
      catalogId: a.playParams?.catalogId ?? it.playParams?.catalogId ?? (String(it.id).startsWith('i.') ? null : it.id),
      hasLyrics: a.hasLyrics ?? null,
      albumId: (/\/album\/[^/]+\/(\d+)/.exec(a.url || '') || [])[1] ?? null, // Go to Album
    };
  };

  const songInfo = (s) => ({
    id: s.id,
    title: s.attributes?.name ?? null,
    artist: s.attributes?.artistName ?? null,
    album: s.attributes?.albumName ?? null,
    duration: s.attributes?.durationInMillis != null ? s.attributes.durationInMillis / 1000 : null,
  });

  // Apple Music API through the page's MusicKit (uses its developer + user tokens).
  // `body` makes it a write (POST unless `method` says otherwise), e.g. adding to a playlist.
  // Writes use fetch with the page's own tokens, as Apple's web app does.
  const apiMusic = async (path, params = {}, method, body) => {
    path = path.replace('{{storefrontId}}', mk.storefrontId);
    if (body === undefined && !method) {
      const res = await mk.api.music(path, params);
      return res?.data ?? null;
    }
    const res = await fetch('https://amp-api.music.apple.com' + path, {
      method: method || 'POST',
      headers: {
        Authorization: `Bearer ${mk.developerToken}`,
        'Music-User-Token': mk.musicUserToken,
        'Content-Type': 'application/json',
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const text = await res.text();
    return text ? JSON.parse(text) : null;
  };

  // MusicKit's `currentPlaybackTime` is whole seconds, too coarse for synced lyrics. The <audio>
  // element MusicKit plays through has the exact time; use it while it agrees with MusicKit (the
  // same song, the same timeline), else fall back. The first pick is logged once, to check it.
  let mediaEl = null;
  let timeSourceLogged = false;
  const playbackTime = () => {
    const coarse = mk?.currentPlaybackTime ?? 0;
    if (!mediaEl || !mediaEl.isConnected) {
      mediaEl = document.getElementById('apple-music-player') || document.querySelector('audio');
    }
    const fine = mediaEl && mediaEl.readyState > 0 ? mediaEl.currentTime : NaN;
    const ok = Number.isFinite(fine) && Math.abs(fine - coarse) < 1.5;
    if (!timeSourceLogged && coarse > 2) {
      timeSourceLogged = true;
      emit({ event: 'log', message: `time source: ${ok ? 'audio element' : 'MusicKit'} (MusicKit ${coarse}, element ${fine})` });
    }
    return ok ? fine : coarse;
  };

  const status = () => ({
    authorized: !!mk.isAuthorized,
    storefront: mk.storefrontId ?? null,
    state: stateName(mk.playbackState),
    nowPlaying: itemInfo(mk.nowPlayingItem),
    current: playbackTime(),
    duration: mk.currentPlaybackDuration ?? 0,
    queueLength: mk.queue?.length ?? 0,
    volume: mk.volume ?? null,
    bitrate: mk.bitrate ?? null,
    bitrates: window.MusicKit && MusicKit.PlaybackBitrate ? { ...MusicKit.PlaybackBitrate } : null,
  });

  // ---- queue
  // A click queues only the clicked song (as fast as a single `play <id>`); the rest of the
  // list waits here and is handed to MusicKit a batch at a time when playback gets near the end
  // of what MusicKit has, or on Next. So MusicKit is idle while the user browses and clicks, and
  // a click never waits behind a long append. Queue changes (mk.setQueue, appends) run one at a
  // time; the newest request wins (older ones are skipped if they haven't started). play() is
  // NOT part of that chain: MusicKit's play() can hang for many seconds, and the newest click
  // must never wait for an older song to start, so it calls mk.setQueue while that play() is
  // still pending (and the older call returns `superseded`).
  let queueGen = 0; // bumps on every setQueue; older work checks it and stops
  let chain = Promise.resolve();
  let pending = []; // song ids after the end of MusicKit's queue, in play order
  let loop = null; // repeat all: the next lap in the order the app shows it, refilled into `pending` when it runs out
  // queueGen when setUpcoming last replaced `pending`. A Play Next / Add to Queue sent while
  // mk.setQueue is still working is newer than the list setQueue arrived with, so it must win.
  let upcomingGen = -1;
  let topping = null; // in-flight top-up
  let timeEveryMs = 1000; // time event interval (`timeRate`)
  let repeatOne = false; // see `setRepeat`
  let looping = false; // a repeat-one jump back is in flight
  // One song at a time, ~30 s before the current one ends: then at most one song is already
  // fixed in MusicKit's queue, so a new upcoming order (shuffle) applies from the next song.
  const BATCH = 1;

  // Never let one stuck MusicKit call wedge the chain.
  const timeout = (promise, ms, what) =>
    Promise.race([promise, new Promise((_, reject) => setTimeout(() => reject(new Error(`${what} timed out`)), ms))]);

  const serial = (fn) => {
    const run = chain.then(fn);
    chain = run.catch(() => {});
    return run;
  };

  // Items left in MusicKit's queue after the current one.
  const ahead = () => {
    const len = mk.queue?.length ?? 0;
    const pos = mk.nowPlayingItemIndex ?? mk.queue?.position ?? 0;
    return Math.max(0, len - 1 - pos);
  };

  function topUp() {
    if (pending.length === 0 && loop && loop.length > 0) pending = loop.slice();
    if (topping || pending.length === 0) return topping;
    const gen = queueGen;
    const batch = pending.splice(0, BATCH);
    topping = serial(async () => {
      if (gen !== queueGen) return;
      await timeout(mk.playLater({ songs: batch }), 15000, 'append');
    })
      .catch((e) => {
        if (gen === queueGen) emit({ event: 'error', message: `couldn't queue some songs: ${e?.message ?? e}` });
      })
      .finally(() => {
        topping = null;
      });
    return topping;
  }

  async function startQueue(args, gen, arrived) {
    const { songIds, station, startPlaying = true, startTime, id } = args;
    // A resumed song starts this far in (seconds). MusicKit's own `startTime` option may do it;
    // if not, `seekToResume` seeks once playback runs.
    const at = Number(startTime) > 0 ? Number(startTime) : null;
    resumeAt = at;
    // Only the queue change is serial; see "queue" above.
    const q = await serial(async () => {
      if (gen !== queueGen) {
        mark(`#${id} superseded`);
        return null;
      }
      const t0 = performance.now();
      const waitMs = Math.round(t0 - arrived);
      mark(`#${id} setQueue start (waited ${waitMs} ms)`);
      const opts = {};
      let rest = [];
      if (Array.isArray(songIds) && songIds.length > 0) {
        const ids = songIds.map(String);
        opts.songs = ids.slice(0, 1);
        rest = ids.slice(1);
      } else if (station) opts.station = String(station);
      else throw new Error('setQueue needs songIds or station');
      if (at !== null) opts.startTime = at;

      await timeout(mk.setQueue(opts), 15000, 'setQueue');
      if (gen !== queueGen) {
        mark(`#${id} superseded`);
        return null;
      }
      if (upcomingGen !== gen) pending = rest;
      const queuedMs = Math.round(performance.now() - t0);
      mark(`#${id} setQueue done (${queuedMs} ms)`);
      return { t0, waitMs, queuedMs };
    });
    // A newer click may have arrived while the chain handed back.
    if (!q || gen !== queueGen) return { superseded: true };
    if (startPlaying) {
      mark(`#${id} play() start`);
      try {
        await timeout(mk.play(), 15000, 'play');
        mark(`#${id} play() resolved (${Math.round(performance.now() - q.t0 - q.queuedMs)} ms)`);
        if (gen !== queueGen) return { superseded: true }; // finished late, after a newer click took over
        await seekToResume();
      } catch (e) {
        if (gen !== queueGen) {
          mark(`#${id} superseded`);
          return { superseded: true }; // the user already replaced this click: no error for it
        }
        mark(`#${id} play() failed: ${e?.message ?? e}`);
        throw e;
      }
    }
    return {
      queueLength: (mk.queue?.length ?? 0) + pending.length,
      waitMs: q.waitMs,
      queuedMs: q.queuedMs,
      playMs: Math.round(performance.now() - q.t0),
    };
  }

  // Seconds a resumed song should start at, until playback runs there (see `startQueue`).
  let resumeAt = null;

  // The account's storefront, once the app has asked Apple (`setStorefront`). The page keeps its
  // URL's ("us") otherwise; see applyStorefront in signin.js. Applied again before playback starts,
  // since music.apple.com sets preview-only again on its own after a sign-in.
  let accountStorefront = null;
  const ensureStorefront = () => {
    if (!accountStorefront || !mk?.isAuthorized || typeof window.__uzumakiApplyStorefront !== 'function') return;
    try {
      const r = window.__uzumakiApplyStorefront(mk, accountStorefront);
      if (r.before !== r.storefront) emit({ event: 'log', message: `storefront ${r.before} -> ${r.storefront} again` });
    } catch {}
  };
  async function seekToResume() {
    const at = resumeAt;
    resumeAt = null;
    if (at === null) return;
    if (Math.abs(playbackTime() - at) > 2) {
      try {
        await timeout(mk.seekToTime(at), 5000, 'seek');
      } catch (e) {
        emit({ event: 'log', message: `resume seek: ${e?.message ?? e}` });
      }
    }
  }

  const commands = {
    status: async () => status(),
    // `at`: a resumed song held paused (setQueue with startPlaying false) starts there.
    play: async ({ at } = {}) => {
      ensureStorefront();
      if (Number(at) >= 0 && at !== undefined && at !== null) resumeAt = Number(at) > 0 ? Number(at) : null;
      await mk.play();
      await seekToResume();
    },
    pause: async () => mk.pause(),
    next: async () => {
      if (ahead() === 0 && (pending.length > 0 || loop)) await topUp();
      return mk.skipToNextItem();
    },
    previous: async () => mk.skipToPreviousItem(),
    seek: async ({ time }) => mk.seekToTime(Number(time)),
    volume: async ({ value }) => {
      mk.volume = Math.max(0, Math.min(1, Number(value)));
    },
    // What plays after what MusicKit already has (the app owns the order: shuffle, play next,
    // add to queue). `loop`: the whole cycle for repeat all, or null. Returns how many songs are
    // already fixed in MusicKit's queue ahead of the current one (0 or 1).
    setUpcoming: async ({ songIds, loop: cycle }) => {
      pending = (songIds || []).map(String);
      upcomingGen = queueGen;
      loop = Array.isArray(cycle) && cycle.length > 0 ? cycle.map(String) : null;
      return { ahead: ahead() };
    },
    // Sends the current song, state and time again as events. After a failed start the app's
    // idea of what plays is wrong, and MusicKit (still on the old queue) reports no change.
    announce: async () => {
      emit({ event: 'nowPlaying', item: itemInfo(mk.nowPlayingItem) });
      emit({ event: 'playbackState', state: stateName(mk.playbackState) });
      emit({ event: 'time', current: playbackTime(), duration: mk.currentPlaybackDuration ?? 0 });
      return null;
    },
    // 'off' | 'one' | 'all'. Repeat one is MusicKit's; repeat all is `loop` above.
    // Repeat one is done here, not with MusicKit's repeatMode: at a track's end MusicKit briefly
    // switched to the next queued song before repeating. We queue nothing next and jump back to
    // the start just before the end, so the song loops without touching another track.
    setRepeat: async ({ mode }) => {
      mk.repeatMode = 0;
      repeatOne = mode === 'one';
      if (mode !== 'all') loop = null;
      return null;
    },
    // Opens Apple's sign-in, as the page's Sign In button does. Doesn't wait for it (the user may
    // take minutes); the `auth` event reports the result.
    authorize: async () => {
      if (!mk.isAuthorized) {
        mk.authorize().catch((err) => emit({ event: 'log', message: `authorize: ${err?.message ?? err}` }));
      }
      return { authorized: !!mk.isAuthorized };
    },
    // Sign out (the main process then clears cookies and reloads). Stops playback first and
    // drops the lazy queue so nothing starts again under the old account.
    unauthorize: async () => {
      queueGen++;
      pending = [];
      loop = null;
      repeatOne = false;
      try {
        await mk.stop();
      } catch {}
      accountStorefront = null;
      await mk.unauthorize();
      return { authorized: !!mk.isAuthorized };
    },
    // The account's storefront (the app asked /v1/me/storefront): catalog calls and playback use it.
    setStorefront: async ({ storefront }) => {
      if (typeof window.__uzumakiApplyStorefront !== 'function') throw new Error('storefront setter not injected');
      const r = window.__uzumakiApplyStorefront(mk, storefront);
      accountStorefront = r.storefront;
      return r;
    },
    // 256 (best, default) or 64 (data saver).
    setBitrate: async ({ kbps }) => {
      mk.bitrate = Number(kbps) === 64 ? (MusicKit.PlaybackBitrate?.STANDARD ?? 64) : (MusicKit.PlaybackBitrate?.HIGH ?? 256);
      return { bitrate: mk.bitrate };
    },
    // Time events per second (1 normally; ~4 while synced lyrics are on screen).
    timeRate: async ({ hz }) => {
      timeEveryMs = 1000 / Math.max(1, Math.min(10, Number(hz) || 1));
      return null;
    },
    // songIds: the clicked song first, then what should follow it (see "queue" above).
    setQueue: (args) => {
      const arrived = performance.now();
      ensureStorefront();
      const gen = ++queueGen;
      pending = [];
      if (args.loop !== undefined) loop = Array.isArray(args.loop) && args.loop.length ? args.loop.map(String) : null;
      return startQueue(args, gen, arrived);
    },
    search: async ({ term, limit = 10 }) => {
      const data = await apiMusic('/v1/catalog/{{storefrontId}}/search', { term, types: 'songs', limit });
      return { songs: (data?.results?.songs?.data ?? []).map(songInfo) };
    },
    // Raw passthrough for anything else (library, playlists, lyrics, ...).
    api: async ({ path, params, method, body }) => apiMusic(path, params, method, body),
  };

  // Debug helper that works before MusicKit exists: can this page get a Widevine key system?
  const widevine = async () => {
    try {
      await navigator.requestMediaKeySystemAccess('com.widevine.alpha', [
        { audioCapabilities: [{ contentType: 'audio/mp4; codecs="mp4a.40.2"' }] },
      ]);
      return { available: true };
    } catch (e) {
      return { available: false, error: String(e?.message ?? e) };
    }
  };

  window.__cadence = {
    async handle(msg) {
      try {
        if (msg.cmd === 'widevine') return { ok: true, result: await widevine() };
        if (!mk) return { ok: false, error: 'MusicKit not ready yet' };
        const fn = commands[msg.cmd];
        if (!fn) return { ok: false, error: `unknown command: ${msg.cmd}` };
        const result = await fn(msg);
        // Round-trip through JSON so the result can cross the world boundary.
        return { ok: true, result: result === undefined ? null : JSON.parse(JSON.stringify(result)) };
      } catch (e) {
        return { ok: false, error: String(e?.message ?? e) };
      }
    },
  };

  function attach(inst) {
    mk = inst;
    // Best quality Apple streams to the web player (AAC 256 kbps). MusicKit may otherwise drop to
    // 64 kbps on its own when it guesses the network is slow. `setBitrate` changes it.
    try {
      inst.bitrate = MusicKit.PlaybackBitrate?.HIGH ?? 256;
    } catch {}
    let lastSecond = -1;
    mark(`MusicKit found: authorized=${!!inst.isAuthorized}, state=${stateName(inst.playbackState)}, queue=${inst.queue?.length ?? 0}, nowPlaying=${inst.nowPlayingItem?.id ?? 'none'}`);
    // Every MusicKit event except the high-frequency ones, by name. Payloads are never logged
    // (userTokenDidChange carries the user token): only the few fields below.
    const names = Object.values((window.MusicKit && MusicKit.Events) || {}).filter((n) => typeof n === 'string');
    for (const name of names.length ? names : [
      'playbackStateDidChange', 'nowPlayingItemWillChange', 'nowPlayingItemDidChange', 'mediaItemStateDidChange',
      'mediaCanPlay', 'queueItemsDidChange', 'queuePositionDidChange', 'authorizationStatusDidChange',
      'mediaPlaybackError', 'playbackBitrateDidChange',
    ]) {
      if (/time|progress|duration|timedMetadata|volume/i.test(name)) continue;
      inst.addEventListener(name, (e) => {
        let detail = '';
        if (/token/i.test(name)) detail = '';
        else if (name === 'playbackStateDidChange') detail = ` ${stateName(inst.playbackState)}`;
        else if (/^nowPlaying/.test(name) || name === 'mediaItemStateDidChange') {
          detail = ` ${inst.nowPlayingItem?.id ?? 'none'}`;
          if (name === 'mediaItemStateDidChange' && typeof e?.state === 'number') detail += ` state=${e.state}`;
        }
        mark('mk ' + name + detail);
      });
    }
    inst.addEventListener('playbackStateDidChange', () => {
      const state = stateName(inst.playbackState);
      if (repeatOne && (state === 'ended' || state === 'completed')) {
        Promise.resolve(inst.seekToTime(0)).then(() => inst.play()).catch(() => {});
        return;
      }
      emit({ event: 'playbackState', state });
    });
    inst.addEventListener('nowPlayingItemDidChange', () =>
      emit({ event: 'nowPlaying', item: itemInfo(inst.nowPlayingItem) }),
    );
    let lastTimeEmit = 0;
    inst.addEventListener('playbackTimeDidChange', () => {
      const t = playbackTime();
      const left = (inst.currentPlaybackDuration ?? 0) - t;
      if (repeatOne) {
        // Time ticks come ~4×/s, so 0.6 s before the end is always caught.
        if (left > 0 && left < 0.6 && !looping) {
          looping = true;
          Promise.resolve(inst.seekToTime(0)).finally(() => {
            looping = false;
          });
        }
      } else if ((pending.length > 0 || loop) && left > 0 && left < 30 && ahead() === 0) topUp();
      const now = performance.now();
      // 1/s normally, faster for synced lyrics. The slack keeps every ~250 ms tick at 4/s.
      if (now - lastTimeEmit < timeEveryMs - 40) return;
      lastTimeEmit = now;
      emit({ event: 'time', current: t, duration: inst.currentPlaybackDuration ?? 0 });
    });
    inst.addEventListener('playbackVolumeDidChange', () => emit({ event: 'volume', value: inst.volume }));
    // The page's storefront (`storefrontId`, its URL's until `setStorefront`). Not the account's:
    // the app asks Apple for that (/v1/me/storefront) and sends `setStorefront`.
    let storefrontSent = inst.storefrontId ?? null;
    inst.addEventListener('authorizationStatusDidChange', () => {
      storefrontSent = inst.storefrontId ?? null;
      emit({ event: 'auth', authorized: !!inst.isAuthorized, storefront: storefrontSent });
    });
    const storefrontChanged = () => {
      const id = inst.storefrontId ?? null;
      if (!id || id === storefrontSent) return;
      storefrontSent = id;
      emit({ event: 'storefront', id });
    };
    for (const name of ['storefrontIdDidChange', 'storefrontCountryCodeDidChange']) {
      try {
        inst.addEventListener(name, storefrontChanged);
      } catch {}
    }
    inst.addEventListener('mediaPlaybackError', (e) =>
      emit({ event: 'error', message: `playback error: ${e?.message ?? e?.name ?? e?.errorCode ?? 'unknown'}` }),
    );
    emit({
      event: 'ready',
      authorized: !!inst.isAuthorized,
      storefront: inst.storefrontId ?? null,
      volume: inst.volume ?? null,
    });
  }

  // music.apple.com configures MusicKit itself; wait for its instance.
  const timer = setInterval(() => {
    let inst = null;
    try {
      inst = window.MusicKit && MusicKit.getInstance();
    } catch {}
    if (inst) {
      clearInterval(timer);
      attach(inst);
    }
  }, 250);
}
