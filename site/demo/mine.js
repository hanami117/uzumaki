/* Uzumaki site, "Make it yours": a small replica window whose wallpaper is drawn by the app's own effect engine
   (fx.wasm, in fx-worker.js: this file only asks it for pictures through UzuFx.render, see fx.js) and real controls for
   the background source, effect, strength and fade. Nothing starts (no worker, no fetch) until the window is near the
   viewport. "Your picture" is decoded here with createImageBitmap and never leaves the browser. Without WebAssembly or
   workers (or with "reduced data") the window shows the plain picture, blurred with CSS, as the demo does. */
(() => {
  const $ = (id) => document.getElementById(id);
  const box = $('mw-box'), win = $('mw'), wall = $('mw-wall'), fb = $('mw-fb'), ctl = $('mine-ctl');
  if (!box || !win || !wall || !ctl) return;
  const fx = window.UzuFx || { supported: false, current: null, render: () => Promise.resolve(null), onTrack() {} };
  const status = $('mine-status'), picRow = $('mine-pic'), fileIn = $('mine-file'), rmBtn = $('mine-rm'), nameEl = $('mine-name');
  const fxDesc = $('mine-fxd'), op = $('mine-op'), opOut = $('mine-opv'), fadeIn = $('mine-fade');
  const W = 700, H = 440;                      // the window's design size
  const SWIRL = new URL('assets/swirl.webp', location.href).href;
  const PIC_MAX = 1600, FILE_MAX = 40 * 1048576;

  // what the controls say; fade on as in the app; the swirl with Halftone at 50% (the app starts at 35%) so there is something to see
  const state = { source: 'swirl', effect: +ctl.querySelector('input[name="mine-fx"]:checked').value, fade: true };
  let near = false, fit = 1, picSeq = 0, rseq = 0, timer = 0, shown = '';
  let pic = null;                              // {id, bitmap, url (object URL, for the CSS fallback), name}
  const cache = new Map();                     // key -> ImageBitmap, newest last
  const CACHE_MAX = 12;
  const closeIt = (b) => { if (b && b.close) b.close(); };

  // ---- two stacked canvases, cross-faded (opacity only); the strength slider sets --o on the wall, so it never re-renders
  const layers = [0, 1].map(() => { const c = document.createElement('canvas'); wall.insertBefore(c, fb); return c; });
  let cur = 0;
  const say = (msg) => { status.textContent = msg || ''; };
  const draw = (bmp, key) => {
    if (key === shown) return;
    const n = layers[1 - cur];
    if (n.width !== bmp.width || n.height !== bmp.height) { n.width = bmp.width; n.height = bmp.height; }
    n.getContext('2d').drawImage(bmp, 0, 0);
    n.classList.add('show'); layers[cur].classList.remove('show');
    cur = 1 - cur; shown = key;
    wall.classList.remove('fb-on');
  };
  const clearWall = () => { shown = ''; layers.forEach((l) => l.classList.remove('show')); wall.classList.remove('fb-on'); };
  const remember = (key, bmp) => {
    const old = cache.get(key); if (old && old !== bmp) closeIt(old);
    cache.delete(key); cache.set(key, bmp);
    while (cache.size > CACHE_MAX) { const k = cache.keys().next().value; closeIt(cache.get(k)); cache.delete(k); }
  };
  const forget = (prefix) => { for (const k of [...cache.keys()]) if (k.startsWith(prefix)) { closeIt(cache.get(k)); cache.delete(k); } };

  // ---- the three sources
  let swirlP = null;
  const swirlBitmap = () => swirlP || (swirlP = (async () => {
    // the swirl on the app's dark ground, composed like the app's built-in picture (centre at 60% / 46%, about the window's diagonal)
    const img = await createImageBitmap(await (await fetch(SWIRL)).blob());
    const c = document.createElement('canvas'); c.width = W * 2; c.height = H * 2;
    const g = c.getContext('2d'); g.fillStyle = '#0e0e11'; g.fillRect(0, 0, c.width, c.height);
    const s = c.height * 1.3; g.drawImage(img, c.width * .6 - s / 2, c.height * .46 - s / 2, s, s);
    img.close();
    return createImageBitmap(c);
  })().catch(() => { swirlP = null; return null; }));
  const coverUrl = () => {
    const t = fx.current; if (t && t.cover) return t.cover;
    const first = document.querySelector('.dw-tile img'); return first ? first.src : '';   // not playing yet: the first track's
  };
  const srcId = () => (state.source === 'swirl' ? 'swirl' : state.source === 'cover' ? 'cover:' + coverUrl() : pic ? 'pic:' + pic.id : '');
  const srcCss = () => (state.source === 'swirl' ? SWIRL : state.source === 'cover' ? coverUrl() : pic ? pic.url : '');

  // ---- show the current choice: cached bitmap at once, else (debounced) ask the worker
  const unit = () => Math.min(2, Math.max(1, Math.round((window.devicePixelRatio || 1) * fit)));
  const keyOf = (id, u) => `${id}|${state.effect}|${W}x${H}@${u}`;

  const fallback = () => {
    const css = srcCss(); if (!css) return clearWall();
    fb.style.backgroundImage = `url("${css}")`;
    wall.classList.toggle('fb-blur', state.effect !== 0);
    clearWall(); wall.classList.add('fb-on');
    if (state.effect !== 0) say('This browser can’t run the effect engine here, so the preview shows the picture softened instead.');
  };

  const request = async (id, key, u) => {
    const seq = ++rseq, effect = state.effect;
    let job;
    if (id === 'swirl') { const bitmap = await swirlBitmap(); if (!bitmap) return seq === rseq && fallback(); job = { url: 'bmp:swirl', bitmap }; }
    else if (id.startsWith('pic:')) { if (!pic || 'pic:' + pic.id !== id) return; job = { url: 'bmp:' + id, bitmap: pic.bitmap }; }
    else job = { url: id.slice(6) };
    const bmp = await fx.render('mine', { ...job, w: W * u, h: H * u, unit: u, effect });
    if (!bmp) { if (seq === rseq) fallback(); return; }
    if (id.startsWith('pic:') && (!pic || 'pic:' + pic.id !== id)) return closeIt(bmp);   // the picture was removed meanwhile
    remember(key, bmp);
    if (seq === rseq && srcId() === id && state.effect === effect) { say(''); draw(bmp, key); }
  };

  const update = () => {
    if (!near) return;                          // applied when the window comes near
    clearTimeout(timer);
    const id = srcId();
    if (!id) { rseq++; clearWall(); say(state.source === 'picture' ? 'Choose a picture to see it here.' : ''); return; }
    if (state.source === 'picture' && pic) say('');
    if (!fx.supported) { rseq++; return fallback(); }
    const u = unit(), key = keyOf(id, u), hit = cache.get(key);
    if (hit) { rseq++; cache.delete(key); cache.set(key, hit); say(''); draw(hit, key); return; }
    timer = setTimeout(() => request(id, key, u), 120);
  };

  // ---- controls
  ctl.hidden = false;
  ctl.addEventListener('submit', (e) => e.preventDefault());
  ctl.addEventListener('change', (e) => {
    const t = e.target;
    if (t.name === 'mine-src') {
      state.source = t.value; picRow.hidden = t.value !== 'picture'; say(''); update();
    } else if (t.name === 'mine-fx') {
      state.effect = +t.value; fxDesc.textContent = t.dataset.desc || ''; say(''); update();
    }
  });
  const setOpacity = () => { const v = +op.value; wall.style.setProperty('--o', (v / 100).toFixed(2)); opOut.textContent = v + '%'; };
  op.addEventListener('input', setOpacity); setOpacity();
  fadeIn.addEventListener('change', () => { state.fade = fadeIn.checked; win.classList.toggle('nofade', !state.fade); });
  fxDesc.textContent = ctl.querySelector('input[name="mine-fx"]:checked').dataset.desc || '';

  // ---- your picture: decoded here, shrunk to at most 1600 px, handed to the worker as a bitmap; no network involved
  const dropPicture = () => {
    if (!pic) return;
    forget('pic:' + pic.id + '|'); closeIt(pic.bitmap); URL.revokeObjectURL(pic.url); pic = null;
    nameEl.textContent = ''; rmBtn.hidden = true;
  };
  const loadPicture = async (file) => {
    if (!file) return;
    if (!(/^image\/(png|jpeg)$/.test(file.type) || /\.(png|jpe?g)$/i.test(file.name))) return say('That isn’t a PNG or JPEG picture.');
    if (file.size > FILE_MAX) return say('That file is too large (40 MB at most).');
    let bmp;
    try { bmp = await createImageBitmap(file, { colorSpaceConversion: 'none', premultiplyAlpha: 'none' }); }
    catch (_) { return say('Couldn’t read that picture.'); }
    const m = Math.max(bmp.width, bmp.height);
    if (m > PIC_MAX) {
      const k = PIC_MAX / m;
      try { const small = await createImageBitmap(bmp, { resizeWidth: Math.max(1, Math.round(bmp.width * k)), resizeHeight: Math.max(1, Math.round(bmp.height * k)), resizeQuality: 'high' }); bmp.close(); bmp = small; }
      catch (_) { /* keep the full-size bitmap */ }
    }
    dropPicture();
    pic = { id: ++picSeq, bitmap: bmp, url: URL.createObjectURL(file), name: file.name };
    nameEl.textContent = file.name; nameEl.title = file.name; rmBtn.hidden = false;
    const radio = ctl.querySelector('input[name="mine-src"][value="picture"]');
    radio.checked = true; state.source = 'picture'; picRow.hidden = false;
    say(''); update();
  };
  fileIn.addEventListener('change', () => { const f = fileIn.files && fileIn.files[0]; fileIn.value = ''; loadPicture(f); });
  rmBtn.addEventListener('click', () => { dropPicture(); say(''); update(); fileIn.focus(); });
  // dropping a picture on the window works too
  let depth = 0;
  const hasFile = (e) => e.dataTransfer && [...e.dataTransfer.types].includes('Files');
  box.addEventListener('dragenter', (e) => { if (hasFile(e)) { depth++; box.classList.add('drop'); } });
  box.addEventListener('dragleave', () => { if (depth && !--depth) box.classList.remove('drop'); });
  box.addEventListener('dragover', (e) => { if (hasFile(e)) e.preventDefault(); });
  box.addEventListener('drop', (e) => { if (!hasFile(e)) return; e.preventDefault(); depth = 0; box.classList.remove('drop'); loadPicture(e.dataTransfer.files[0]); });

  // ---- theme: the author's two screenshots cross-fade (opacity only; the images load lazily on their own)
  const shot = $('mine-shot'), themeBox = $('mine-theme');
  if (shot && themeBox) {
    themeBox.hidden = false;
    themeBox.addEventListener('change', (e) => {
      if (e.target.name !== 'mine-theme') return;
      shot.dataset.theme = e.target.value;
      shot.querySelector('.s-dark').setAttribute('aria-hidden', e.target.value !== 'dark');
      shot.querySelector('.s-light').setAttribute('aria-hidden', e.target.value !== 'light');
    });
  }

  // ---- scale, visibility, the playing song
  let rt = 0;
  const measure = () => {
    const f = box.clientWidth / W; if (!f) return;
    box.style.setProperty('--fit', f.toFixed(4));
    if (Math.abs(f - fit) > .001) { fit = f; clearTimeout(rt); rt = setTimeout(update, 250); }
  };
  if (typeof ResizeObserver === 'function') new ResizeObserver(measure).observe(box); else addEventListener('resize', measure);
  measure();
  let lastCover = coverUrl();
  fx.onTrack(() => { const c = coverUrl(); if (c !== lastCover) { lastCover = c; if (state.source === 'cover') update(); } });
  if (typeof IntersectionObserver === 'function') {
    new IntersectionObserver((es) => { const v = es.some((e) => e.isIntersecting); if (v && !near) { near = true; update(); } }, { rootMargin: '300px 0px' }).observe(box);
  } else { near = true; update(); }
})();
