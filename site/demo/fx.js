/* Uzumaki demo: the app's real background effects (uzumaki-fx, compiled to fx.wasm) behind the replica
   window's wallpaper and Now Playing, plus the gear popover that picks the effect (app: Settings > Appearance).
   All pixel work runs in fx-worker.js; this file only sizes requests, caches results, draws bitmaps to canvases
   and cross-fades them. Without WebAssembly, Workers, or with "reduced data", nothing here starts and the
   blurred-cover CSS fallback stays. Loaded before player.js, which calls UzuFx.setTrack(track). */
(() => {
  // ids are uzumaki_fx::Effect::ALL order; labels are Effect::label (what Settings > Appearance shows)
  const EFFECTS = ['None', 'Dither', 'ASCII', 'Halftone', 'Scanlines', 'Blur'];
  const win = document.getElementById('demo-window');
  // Shared with mine.js ("Make it yours"): the playing track, and render() = one more job for the same worker.
  const trackListeners = [];
  const api = window.UzuFx = {
    supported: false, stats: [], EFFECTS, current: null,
    setTrack(t) { if (t && t.cover) { api.current = t; trackListeners.forEach((f) => f(t)); } },
    onTrack(f) { trackListeners.push(f); },
    render() { return Promise.resolve(null); },
  };
  if (!win) return;
  const wallBox = document.getElementById('dw-wall'), npBox = document.getElementById('np-bg');
  const gears = [...win.querySelectorAll('.dw-gear')];   // the full sidebar's and the rail's
  const activeGear = () => gears.find((g) => !g.closest('[inert]')) || gears[0];
  if (!wallBox || !npBox) return;
  const DEBUG = /[?&]fxdebug\b/.test(location.search);

  const KEY = 'uzumaki-demo-fx', DEFAULT = 3;   // Halftone
  const load = () => { try { const v = EFFECTS.indexOf(localStorage.getItem(KEY)); return v >= 0 ? v : DEFAULT; } catch (_) { return DEFAULT; } };
  let effect = load();

  const ok = typeof Worker === 'function' && typeof WebAssembly === 'object' && typeof createImageBitmap === 'function' && typeof IntersectionObserver === 'function'
    && !matchMedia('(prefers-reduced-data: reduce)').matches && !(navigator.connection && navigator.connection.saveData);
  if (!ok) return;
  api.supported = true;

  // ---- what the app puts over the picture (app/src/ui/backdrop.rs), as CSS in demo.css: the wallpaper at opacity .35 fading
  // into the background colour towards the bottom; Now Playing's "Cover" mode at opacity 1 under a black scrim.
  const slots = {
    wall: { box: wallBox, layers: [], cur: 0, latest: 0, inflight: '', key: '' },
    np: { box: npBox, layers: [], cur: 0, latest: 0, inflight: '', key: '' },
  };
  for (const s of Object.values(slots)) {
    for (let i = 0; i < 2; i++) { const c = document.createElement('canvas'); c.className = 'fxl'; c.setAttribute('aria-hidden', 'true'); s.box.append(c); s.layers.push(c); }
  }

  const ext = new Map();   // api.render jobs in flight: job id -> {slot, res}
  const extDone = (slot, id, val) => { for (const [i, e] of ext) if (e.slot === slot && i <= id) { ext.delete(i); e.res(i === id ? val : null); } };   // older ones were dropped by the worker
  let worker = null, ready = false, dead = false, visible = false, track = null, seq = 0;
  const cache = new Map();   // key -> ImageBitmap, newest last
  const CACHE_MAX = 10;

  const scale = () => {   // device pixels per design pixel, like the app's `unit` (display scale rounded), capped at 2
    const fit = parseFloat(getComputedStyle(win).getPropertyValue('--fit')) || 1;
    return Math.min(2, Math.max(1, Math.round((window.devicePixelRatio || 1) * fit)));
  };
  const keyOf = (name, unit) => { const b = slots[name].box; return `${name}|${track.id}|${effect}|${b.offsetWidth}x${b.offsetHeight}@${unit}`; };

  const setOn = (s, on) => { s.box.classList.toggle('fx-on', on); if (!on) { s.key = ''; s.layers.forEach((l) => l.classList.remove('show')); } };
  const draw = (s, bmp, key) => {
    if (s.key === key) return;
    const n = s.layers[1 - s.cur], c = n.getContext('2d');
    if (n.width !== bmp.width || n.height !== bmp.height) { n.width = bmp.width; n.height = bmp.height; }
    c.drawImage(bmp, 0, 0);
    n.classList.add('show'); s.layers[s.cur].classList.remove('show');
    s.cur = 1 - s.cur; s.key = key; setOn(s, true);
  };
  const remember = (key, bmp) => {
    cache.delete(key); cache.set(key, bmp);
    while (cache.size > CACHE_MAX) { const k = cache.keys().next().value; cache.get(k).close(); cache.delete(k); }
  };

  const request = (name) => {
    const s = slots[name];
    if (!track || !visible || dead) return;
    const b = s.box; if (!b.offsetWidth || !b.offsetHeight) return;
    const unit = scale(), key = keyOf(name, unit);
    const hit = cache.get(key);
    if (hit) { cache.delete(key); cache.set(key, hit); s.inflight = ''; draw(s, hit, key); return; }
    if (s.inflight === key) return;
    s.inflight = key; s.latest = ++seq;
    start(); if (!worker) return;
    worker.postMessage({ slot: name, id: s.latest, url: track.cover, w: b.offsetWidth * unit, h: b.offsetHeight * unit, unit, effect });
    s.sent = performance.now(); s.sentKey = key;
  };
  const requestAll = () => { request('wall'); request('np'); };

  const fail = () => {
    if (dead) return; dead = true; api.supported = false;
    if (worker) worker.terminate(); worker = null;
    ext.forEach((e) => e.res(null)); ext.clear();
    Object.values(slots).forEach((s) => { setOn(s, false); s.layers.forEach((l) => l.remove()); });
    cache.forEach((b) => b.close()); cache.clear();
    popSet(false); gears.forEach((g) => { g.disabled = true; g.setAttribute('aria-hidden', 'true'); g.tabIndex = -1; });
  };

  const onMessage = (e) => {
    const m = e.data;
    if (m.ready) { ready = true; return; }
    if (m.error) {
      if (DEBUG) console.warn('[fx]', m.error);
      if (m.fatal) return fail();
      if (ext.has(m.id)) return extDone(m.slot, m.id, null);
      const s = slots[m.slot]; if (s) { s.inflight = ''; setOn(s, false); }   // this track: blurred cover instead
      return;
    }
    let bmp = m.bitmap;
    if (!bmp) { const c = document.createElement('canvas'); c.width = m.w; c.height = m.h; c.getContext('2d').putImageData(new ImageData(new Uint8ClampedArray(m.rgba), m.w, m.h), 0, 0); bmp = c; }
    if (ext.has(m.id)) return extDone(m.slot, m.id, bmp);   // the caller owns the bitmap
    const s = slots[m.slot];
    const stat = { slot: m.slot, effect: EFFECTS[effect], size: `${m.w}x${m.h}`, fetch: +m.ms.fetch.toFixed(1), decode: +m.ms.decode.toFixed(1), render: +m.ms.render.toFixed(1), total: +(performance.now() - s.sent).toFixed(1) };
    api.stats.push(stat); if (api.stats.length > 60) api.stats.shift();
    if (DEBUG) console.debug('[fx]', JSON.stringify(stat));
    if (m.id === s.latest) {
      const key = s.sentKey; s.inflight = '';
      if (bmp.close) remember(key, bmp);
      if (track && key === keyOf(m.slot, scale())) draw(s, bmp, key); else request(m.slot);
    } else if (bmp.close) bmp.close();   // superseded by a newer request
  };

  function start() {
    if (worker || dead) return;
    try { worker = new Worker('demo/fx-worker.js'); } catch (_) { return fail(); }
    worker.onmessage = onMessage;
    worker.onerror = () => fail();
  }

  const noteTrack = api.setTrack;
  api.setTrack = (t) => {
    if (!t || !t.cover) return;
    noteTrack(t); track = t; requestAll();
  };
  // job: {url, w, h, unit, effect, bitmap?}; resolves to an ImageBitmap (or canvas) the caller owns, or null (failed, or superseded)
  api.render = (slot, job) => new Promise((res) => {
    if (dead) return res(null);
    start(); if (!worker) return res(null);
    const id = ++seq; ext.set(id, { slot, res });
    worker.postMessage({ ...job, slot, id });
  });

  // nothing starts until the window is near the viewport; changes while it is away are applied when it comes back
  new IntersectionObserver((es) => {
    visible = es.some((e) => e.isIntersecting);
    if (visible) requestAll();
  }, { rootMargin: '300px 0px' }).observe(win);
  let rt = 0;
  addEventListener('resize', () => { clearTimeout(rt); rt = setTimeout(requestAll, 250); });

  // ---- gear popover (sidebar): one effect for both backgrounds
  let pop = null, radios = [];
  const popSet = (open) => {
    if (!pop) { if (!open) return; buildPop(); }
    pop.hidden = !open; gears.forEach((g) => g.setAttribute('aria-expanded', open));
    if (open) { const cur = radios[effect]; cur.tabIndex = 0; cur.focus({ preventScroll: true }); }
  };
  const choose = (i) => {
    effect = i; try { localStorage.setItem(KEY, EFFECTS[i]); } catch (_) {}
    radios.forEach((r, j) => { r.setAttribute('aria-checked', j === i); r.tabIndex = j === i ? 0 : -1; });
    requestAll();
  };
  function buildPop() {
    pop = document.createElement('div');
    pop.className = 'dw-fxpop'; pop.id = 'dw-fxpop'; pop.hidden = true; pop.setAttribute('role', 'radiogroup'); pop.setAttribute('aria-label', 'Background effect');
    pop.innerHTML = '<div class="dw-fxh">Background effect</div>';
    radios = EFFECTS.map((name, i) => {
      const b = document.createElement('button'); b.type = 'button'; b.className = 'dw-fxopt'; b.textContent = name;
      b.setAttribute('role', 'radio'); b.setAttribute('aria-checked', i === effect); b.tabIndex = i === effect ? 0 : -1;
      b.addEventListener('click', () => choose(i));
      pop.append(b); return b;
    });
    const note = document.createElement('p'); note.className = 'dw-fxnote'; note.textContent = 'Drawn by the app’s own effect engine, running here as WebAssembly. Applies to the window and Now Playing.';
    pop.append(note);
    pop.addEventListener('keydown', (e) => {
      const d = e.key === 'ArrowDown' || e.key === 'ArrowRight' ? 1 : e.key === 'ArrowUp' || e.key === 'ArrowLeft' ? -1 : 0;
      if (!d) return;
      e.preventDefault(); e.stopPropagation();
      const i = (radios.indexOf(document.activeElement) + d + radios.length) % radios.length;
      radios.forEach((r, j) => { r.tabIndex = j === i ? 0 : -1; }); radios[i].focus();
    });
    win.append(pop);
  }
  if (gears.length) {
    for (const gear of gears) {
      gear.disabled = false; gear.removeAttribute('aria-hidden'); gear.removeAttribute('tabindex');
      gear.setAttribute('aria-label', 'Appearance: background effect'); gear.title = 'Appearance: background effect'; gear.setAttribute('aria-haspopup', 'true'); gear.setAttribute('aria-expanded', 'false'); gear.setAttribute('aria-controls', 'dw-fxpop');
      gear.addEventListener('click', (e) => { e.stopPropagation(); popSet(!pop || pop.hidden); });
    }
    win.addEventListener('click', (e) => { if (pop && !pop.hidden && !pop.contains(e.target)) popSet(false); });
    win.addEventListener('keydown', (e) => { if (e.key === 'Escape' && pop && !pop.hidden) { e.preventDefault(); e.stopPropagation(); popSet(false); activeGear().focus({ preventScroll: true }); } }, true);
  }
})();
