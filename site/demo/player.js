/* Uzumaki demo, stage 2: real 30 s previews in the replica window. Vanilla, no autoplay. */
(() => {
  const $ = (id) => document.getElementById(id);
  const win = $('demo-window'); if (!win) return;
  const wall = $('dw-wall'), cover = $('dw-cover'), titleEl = $('dw-title'), artistEl = $('dw-artist');
  // the capsule and Now Playing carry the same controls (data-act); one state drives both
  const acts = (a) => [...win.querySelectorAll(`[data-act="${a}"]`)];
  const playBtns = acts('play'), shufBtns = acts('shuffle'), prevBtns = acts('prev'), nextBtns = acts('next'), repBtns = acts('repeat');
  const cur = $('dw-cur'), dur = $('dw-dur'), fill = $('dw-fill'), seek = $('dw-seek'), vol = $('dw-vol');
  const npCur = $('np-cur'), npDur = $('np-dur'), npFill = $('np-fill'), npSeek = $('np-seek'), npVol = $('np-vol');
  const seeks = [seek, npSeek], vols = [vol, npVol];
  const npEl = $('dw-np'), npBg = $('np-bg'), npCover = $('np-cover'), npTitle = $('np-title'), npArtist = $('np-artist');
  const opener = $('dw-open'), npClose = $('np-close');
  const credit = $('dw-credit'), toastEl = $('dw-toast'), volBtn = $('dw-volbtn'), pop = $('dw-pop');
  const fx = window.UzuFx || { setTrack() {} };
  const audio = new Audio(); audio.preload = 'none';
  const ICON = (n) => `<use href="#i-${n}"/>`;
  const fmt = (t) => { t = Math.max(0, Math.floor(t || 0)); return `${Math.floor(t / 60)}:${String(t % 60).padStart(2, '0')}`; };

  let tracks = new Map(), queue = [], order = [], pos = -1, shuffle = false, repeat = 0 /* 0 off, 1 all, 2 one */, shelf = [];
  // Up Next (panels.js reads this through window.UzuQueue): the songs heard this session (oldest first, as the app's History),
  // the next lap under repeat all (decided up front, so what Up Next shows after "Repeat" is what plays), the list's name.
  let history = [], heard = null, lapOrder = null, source = '';
  const listeners = [];
  const emit = () => listeners.forEach((f) => f());
  let srcShelf = $('dw-shelf'), raf = 0, dragging = false, started = false, toastT = 0, failed = 0;

  const toast = (msg) => { toastEl.textContent = msg; toastEl.classList.add('show'); clearTimeout(toastT); toastT = setTimeout(() => toastEl.classList.remove('show'), 2600); };
  const line = (t) => (t.album ? `${t.artist} \u2014 ${t.album}` : t.artist);
  const playable = (t) => t && t.previewUrl;
  const curTrack = () => (pos >= 0 ? tracks.get(order[pos]) : null);

  // ---- crossfade: two stacked layers, opacity only. The wallpaper, Now Playing's background and its cover use it.
  const crossfade = (box, make, set, prepend) => {
    const ls = [make(), make()]; ls.forEach((l) => box[prepend ? 'prepend' : 'append'](l));
    let f = 0;
    return (url) => { if (!url) return; const n = ls[1 - f]; set(n, url); n.classList.add('show'); ls[f].classList.remove('show'); f = 1 - f; };
  };
  const bgLayer = () => { const d = document.createElement('div'); d.className = 'wl'; return d; };
  const setBg = (l, url) => { l.style.backgroundImage = `url("${url}")`; };
  const setWall = crossfade(wall, bgLayer, setBg, true);
  const setNpBg = crossfade(npBg, bgLayer, setBg);
  const setNpCover = crossfade(npCover, () => { const i = document.createElement('img'); i.alt = ''; i.decoding = 'async'; return i; }, (l, url) => { l.src = url; });

  // ---- UI sync
  const setPlayIcon = (playing) => {
    playBtns.forEach((b) => { b.querySelector('svg').innerHTML = ICON(playing ? 'pause' : 'play'); b.setAttribute('aria-label', playing ? 'Pause' : 'Play'); });
    win.querySelectorAll('.dw-tile.on').forEach((t) => t.classList.toggle('playing', playing));
  };
  const showTrack = (t) => {
    if (heard && heard !== t.id) { history = history.filter((x) => x !== heard); history.push(heard); if (history.length > 50) history.shift(); }
    history = history.filter((x) => x !== t.id); heard = t.id;
    titleEl.textContent = t.title; artistEl.textContent = line(t); cover.src = t.cover; cover.alt = '';
    win.querySelectorAll('.dw-tile').forEach((el) => el.classList.toggle('on', el.parentNode === srcShelf && el.dataset.track === t.id));
    setWall(t.cover); setNpBg(t.cover); setNpCover(t.cover); fx.setTrack(t); npTitle.textContent = t.title; npArtist.textContent = line(t);
    dur.textContent = '\u22120:30'; seeks.forEach((e) => e.setAttribute('aria-valuemax', '30')); npDur.textContent = '0:30';
    if (t.trackViewUrl) { credit.href = t.trackViewUrl; credit.hidden = false; credit.setAttribute('aria-label', `Preview courtesy of Apple Music: open ${t.title} on Apple Music`); }
    setSession(t);
  };
  const duration = () => (isFinite(audio.duration) && audio.duration > 0 ? audio.duration : 30);
  const paint = () => {
    const d = duration(), c = dragging ? dragT : audio.currentTime;
    const sx = `scaleX(${Math.min(1, c / d).toFixed(4)})`;
    fill.style.transform = sx; npFill.style.transform = sx;
    cur.textContent = fmt(c); dur.textContent = '\u2212' + fmt(Math.max(0, d - c));
    npCur.textContent = fmt(c); npDur.textContent = fmt(d);
    seeks.forEach((e) => { e.setAttribute('aria-valuenow', Math.floor(c)); e.setAttribute('aria-valuemax', Math.floor(d)); });
  };
  const loop = () => { paint(); raf = requestAnimationFrame(loop); };
  const startLoop = () => { if (!raf) raf = requestAnimationFrame(loop); };
  const stopLoop = () => { cancelAnimationFrame(raf); raf = 0; paint(); };

  // ---- queue
  const shuffled = (ids, first) => {
    const rest = ids.filter((id) => id !== first);
    for (let i = rest.length - 1; i > 0; i--) { const j = Math.floor(Math.random() * (i + 1)); [rest[i], rest[j]] = [rest[j], rest[i]]; }
    return first ? [first, ...rest] : rest;
  };
  const planLap = () => { lapOrder = repeat === 1 && queue.length ? (shuffle ? shuffled(queue, null) : queue.slice()) : null; };
  const buildOrder = (ids, startId) => {
    queue = ids.filter((id) => playable(tracks.get(id)));
    if (shuffle) { order = shuffled(queue, startId); pos = 0; }
    else { order = queue.slice(); pos = Math.max(0, order.indexOf(startId)); }
    planLap();
  };
  const load = (play) => {
    const t = curTrack(); if (!t) return;
    started = true; showTrack(t); emit();
    audio.src = t.previewUrl; cur.textContent = npCur.textContent = "0:00"; fill.style.transform = npFill.style.transform = "scaleX(0)";
    if (play) audio.play().catch(onFail);
  };
  const playFromShelf = (ids, id, from) => {
    const t = tracks.get(id);
    if (!playable(t)) return;
    failed = 0; if (from) srcShelf = from;
    const h = srcShelf && srcShelf.previousElementSibling; source = h && h.tagName === 'H3' ? h.textContent.trim() : '';
    buildOrder(ids, id); load(true);
  };
  const next = (auto) => {
    if (!order.length) return;
    if (auto && repeat === 2) { audio.currentTime = 0; audio.play().catch(onFail); return; }
    if (pos + 1 >= order.length) {
      if (repeat === 1 || !auto) { if (repeat === 1 && lapOrder) { order = lapOrder; planLap(); } pos = 0; load(true); return; }
      pos = 0; load(false); setPlayIcon(false); return;   // end of queue: back to first, paused
    }
    pos++; load(true);
  };
  const prev = () => {
    if (!order.length) return;
    if (audio.currentTime > 3) { audio.currentTime = 0; paint(); return; }
    pos = pos > 0 ? pos - 1 : (repeat === 1 ? order.length - 1 : 0); load(true);
  };
  const toggle = () => {
    if (!started) { const id = shelf[0]; if (id) playFromShelf(shelf, id, $('dw-shelf')); return; }
    audio.paused ? audio.play().catch(onFail) : audio.pause();
  };
  const onFail = (e) => {
    if (e && e.name === 'AbortError') return;
    if (e && e.name === 'NotAllowedError') { setPlayIcon(false); return; }
    toast('Couldn’t load that preview, skipping'); failed++;
    if (failed < order.length) setTimeout(() => next(false), 600); else { setPlayIcon(false); }
  };

  audio.addEventListener('play', () => { setPlayIcon(true); startLoop(); if ('mediaSession' in navigator) navigator.mediaSession.playbackState = 'playing'; });
  audio.addEventListener('pause', () => { setPlayIcon(false); stopLoop(); if ('mediaSession' in navigator) navigator.mediaSession.playbackState = 'paused'; });
  audio.addEventListener('playing', () => { failed = 0; });
  audio.addEventListener('ended', () => next(true));
  audio.addEventListener('error', () => onFail({ name: 'MediaError' }));
  audio.addEventListener('loadedmetadata', paint);
  audio.addEventListener('seeked', paint);

  // ---- controls
  playBtns.forEach((b) => b.addEventListener('click', toggle));
  nextBtns.forEach((b) => b.addEventListener('click', () => (started ? next(false) : toggle())));
  prevBtns.forEach((b) => b.addEventListener('click', () => (started ? prev() : toggle())));
  shufBtns.forEach((b) => b.addEventListener('click', () => {
    shuffle = !shuffle;
    shufBtns.forEach((x) => { x.classList.toggle('on', shuffle); x.setAttribute('aria-pressed', shuffle); x.setAttribute('aria-label', shuffle ? 'Shuffle on' : 'Shuffle off'); });
    if (started) { const id = order[pos]; buildOrder(queue, id); }
    emit();
  }));
  repBtns.forEach((b) => b.addEventListener('click', () => {
    repeat = (repeat + 1) % 3; planLap(); emit();
    repBtns.forEach((x) => { x.classList.toggle('on', repeat > 0); x.classList.toggle('one', repeat === 2); x.setAttribute('aria-label', ['Repeat off', 'Repeat all', 'Repeat one'][repeat]); x.setAttribute('aria-pressed', repeat > 0 ? (repeat === 2 ? 'mixed' : 'true') : 'false'); });
  }));

  // sliders: pointer capture, drag + click
  const slider = (el, onMove, onEnd) => {
    const frac = (e) => { const r = el.getBoundingClientRect(); return Math.min(1, Math.max(0, (e.clientX - r.left) / r.width)); };
    el.addEventListener('pointerdown', (e) => { el.setPointerCapture(e.pointerId); onMove(frac(e), true); });
    el.addEventListener('pointermove', (e) => { if (el.hasPointerCapture(e.pointerId)) onMove(frac(e), false); });
    el.addEventListener('pointerup', (e) => { if (el.hasPointerCapture(e.pointerId)) { onEnd(frac(e)); el.releasePointerCapture(e.pointerId); } });
    el.addEventListener('pointercancel', () => onEnd(null));
    el.addEventListener('lostpointercapture', () => onEnd(null));
  };
  let dragT = 0;
  seeks.forEach((e) => slider(e, (f) => { if (!started) return; dragging = true; dragT = f * duration(); paint(); },
    (f) => { if (!dragging) return; dragging = false; if (f != null) audio.currentTime = f * duration(); paint(); }));
  const setVol = (v) => {
    v = Math.min(1, Math.max(0, v)); audio.volume = v;
    vols.forEach((e) => { e.querySelector('i').style.transform = `scaleX(${v})`; e.setAttribute('aria-valuenow', Math.round(v * 100)); });
  };
  setVol(.7);
  vols.forEach((e) => slider(e, (f) => setVol(f), () => {}));
  const keySlider = (el, get, set, step) => el.addEventListener('keydown', (e) => {
    const d = e.key === 'ArrowRight' || e.key === 'ArrowUp' ? 1 : e.key === 'ArrowLeft' || e.key === 'ArrowDown' ? -1 : 0;
    if (!d) return; e.preventDefault(); e.stopPropagation(); set(get() + d * step);
  });
  seeks.forEach((e) => keySlider(e, () => audio.currentTime, (t) => { if (started) { audio.currentTime = Math.min(duration(), Math.max(0, t)); paint(); } }, 5));
  vols.forEach((e) => keySlider(e, () => audio.volume, setVol, .05));

  // ---- Now Playing: slides up over the window (below the title bar); same state, same controls
  const inertOthers = (on) => win.querySelectorAll('.dw-body, .dw-strip').forEach((e) => { e.inert = on; });
  let npOpen = false;
  const setNp = (open) => {
    if (open === npOpen) return;
    npOpen = open; setPop(false);
    opener.setAttribute('aria-expanded', open);
    if (open) {
      inertOthers(true); npEl.inert = false; npEl.classList.add('open');
      npClose.focus({ preventScroll: true });
    } else {
      npEl.classList.remove('open'); npEl.inert = true; inertOthers(false);
      opener.focus({ preventScroll: true });
    }
  };
  // clicking the cover/title opens it (starting the first preview if nothing has played yet); a second click closes it
  const toggleNp = () => { if (npOpen) return setNp(false); if (!started) toggle(); if (started) setNp(true); };
  opener.addEventListener('click', toggleNp);
  opener.addEventListener('keydown', (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); e.stopPropagation(); toggleNp(); } });
  npClose.addEventListener('click', () => setNp(false));
  document.addEventListener('keydown', (e) => { if (e.key === 'Escape' && npOpen && pop.hidden) { e.preventDefault(); setNp(false); } });


  // volume popover (the capsule has a volume button, as in the app)
  const setPop = (open) => { pop.hidden = !open; volBtn.setAttribute('aria-expanded', open); };
  volBtn.addEventListener('click', (e) => { e.stopPropagation(); setPop(pop.hidden); });
  // a drag that starts on the volume slider must not count as an outside click when it ends elsewhere
  let popGesture = false;
  win.addEventListener('pointerdown', (e) => { popGesture = !pop.hidden && pop.contains(e.target); }, true);
  win.addEventListener('click', (e) => { if (!pop.hidden && !popGesture && !pop.contains(e.target)) setPop(false); });
  win.addEventListener('keydown', (e) => { if (e.key === 'Escape' && !pop.hidden) { setPop(false); volBtn.focus(); } });

  // Space toggles when focus is inside the window (buttons keep their own Space)
  win.addEventListener('keydown', (e) => {
    if (e.code !== 'Space' && e.key !== ' ') return;
    const t = e.target;
    if (t.closest && t.closest('button, a, .dw-tile, [role="button"], [role="separator"]')) return;
    e.preventDefault(); toggle();
  });

  // tiles
  const shelfIds = (el) => [...el.closest('.dw-shelf').querySelectorAll('.dw-tile')].map((x) => x.dataset.track);
  win.addEventListener('click', (e) => { const t = e.target.closest('.dw-tile'); if (t) playFromShelf(shelfIds(t), t.dataset.track, t.closest('.dw-shelf')); });
  win.addEventListener('keydown', (e) => {
    const t = e.target.closest && e.target.closest('.dw-tile');
    if (t && (e.key === 'Enter' || e.key === ' ')) { e.preventDefault(); e.stopPropagation(); playFromShelf(shelfIds(t), t.dataset.track, t.closest('.dw-shelf')); }
  });

  // ---- Up Next model for panels.js: read-only snapshot + the app's row actions (play_up_next, play_history, remove_up_next)
  const T = (id) => tracks.get(id);
  window.UzuQueue = {
    on: (f) => listeners.push(f),
    state: () => ({
      started, shuffle, repeat, source, current: started ? curTrack() : null,
      history: history.map(T).filter(Boolean),
      upcoming: started ? order.slice(pos + 1).map(T).filter(Boolean) : [],
      lap: started && lapOrder ? lapOrder.map(T).filter(Boolean) : [],
    }),
    jump(j) { if (started && pos + 1 + j < order.length) { pos = pos + 1 + j; load(true); } },
    jumpLap(k) { if (started && lapOrder && k < lapOrder.length) { order = lapOrder; pos = k; planLap(); load(true); } },
    playHistory(id) {
      if (!playable(T(id))) return;
      const i = order.indexOf(id);
      if (i >= 0) { pos = i; planLap(); } else { source = 'History'; buildOrder([id], id); }
      failed = 0; load(true);
    },
    remove(j) {
      const i = pos + 1 + j; if (!started || i >= order.length) return;
      const [id] = order.splice(i, 1), q = queue.indexOf(id); if (q >= 0) queue.splice(q, 1);
      planLap(); emit();
    },
  };

  // ---- Media Session
  function setSession(t) {
    if (!('mediaSession' in navigator) || typeof MediaMetadata === 'undefined') return;
    navigator.mediaSession.metadata = new MediaMetadata({ title: t.title, artist: t.artist, album: t.album,
      artwork: [{ src: new URL(t.cover, location.href).href, sizes: '600x600', type: 'image/jpeg' }] });
  }
  if ('mediaSession' in navigator) {
    const h = (a, f) => { try { navigator.mediaSession.setActionHandler(a, f); } catch (_) {} };
    h('play', () => { started ? audio.play().catch(onFail) : toggle(); });
    h('pause', () => audio.pause());
    h('previoustrack', () => prev());
    h('nexttrack', () => next(false));
    h('seekto', (d) => { if (started && d.seekTime != null) { audio.currentTime = d.seekTime; paint(); } });
  }

  // ---- boot
  fetch('demo/tracks.json').then((r) => r.json()).then((j) => {
    j.tracks.forEach((t) => tracks.set(t.id, t));
    shelf = [...$('dw-shelf').querySelectorAll('.dw-tile')].map((x) => x.dataset.track);
    const first = tracks.get(shelf[0]);
    if (first) { titleEl.textContent = first.title; artistEl.textContent = line(first); cover.src = first.cover; setWall(first.cover); setNpBg(first.cover); setNpCover(first.cover); fx.setTrack(first); npTitle.textContent = first.title; npArtist.textContent = line(first); }
    paint();
  }).catch(() => {});
})();
