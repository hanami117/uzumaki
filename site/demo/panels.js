/* Uzumaki demo, stage 5: the sidebars, as in the app.
   - left sidebar: collapses to the icon rail (the panel button beside the gear), drag its edge to resize, width + collapsed saved
   - right panel (app: right_panel.rs): Lyrics | Up Next, opened by the player's Lyrics / Up Next buttons, closed by its X or Escape
   - Up Next (app: up_next.rs): History above, Now Playing, the rest of the list, the next lap after "Repeat"; rows play on click
   - Now Playing's queue button puts the same list beside the cover
   The page-level layout (flex row, the panel sliding by transform) lives in demo.css; the song model is player.js's window.UzuQueue. */
(() => {
  const $ = (id) => document.getElementById(id);
  const win = $('demo-window'); if (!win) return;
  const clamp = (v, a, b) => Math.min(b, Math.max(a, v));
  const reduced = matchMedia('(prefers-reduced-motion: reduce)');
  const Q = window.UzuQueue;

  // ================= left sidebar =================
  // The app's sidebar is a fixed 232 pt (76 pt as the rail); the replica draws the app at 0.862 x (200 / 66). Dragging the edge is
  // this page's own extra, so its limits are ours too.
  const SB_DEF = 200, SB_MIN = 170, SB_MAX = 300;
  const KEY = 'uzumaki-demo-ui';
  const full = $('dw-sbfull'), rail = $('dw-rail'), grip = $('dw-grip');
  const toggles = [...win.querySelectorAll('[data-sbtoggle]')];
  let ui = { w: SB_DEF, collapsed: false };
  try { const j = JSON.parse(localStorage.getItem(KEY) || '{}'); if (typeof j.w === 'number') ui.w = clamp(Math.round(j.w), SB_MIN, SB_MAX); ui.collapsed = j.collapsed === true; } catch (_) {}
  const save = () => { try { localStorage.setItem(KEY, JSON.stringify(ui)); } catch (_) {} };

  const applySb = () => {
    win.style.setProperty('--sbw-exp', ui.w + 'px');
    win.classList.toggle('sb-collapsed', ui.collapsed);
    full.inert = ui.collapsed; rail.inert = !ui.collapsed;
    toggles.forEach((b) => b.setAttribute('aria-expanded', String(!ui.collapsed)));
    grip.setAttribute('aria-valuenow', ui.w);
  };
  win.classList.add('no-anim'); applySb();
  requestAnimationFrame(() => requestAnimationFrame(() => win.classList.remove('no-anim')));

  const setCollapsed = (c) => {
    if (ui.collapsed === c) return;
    ui.collapsed = c; applySb(); save();
    const t = toggles.find((b) => !b.closest('[inert]'));   // the other view's button takes the focus
    if (t) t.focus({ preventScroll: true });
  };
  toggles.forEach((b) => b.addEventListener('click', () => setCollapsed(!ui.collapsed)));

  // drag the edge. Pointer deltas are in screen px: divide by the window's on-screen scale (--fit, tilt) to get design px.
  let drag = null, dragRaf = 0, pendingW = 0;
  const setW = (w, persist) => {
    ui.w = clamp(Math.round(w), SB_MIN, SB_MAX);
    pendingW = ui.w;
    if (!dragRaf) dragRaf = requestAnimationFrame(() => { dragRaf = 0; win.style.setProperty('--sbw-exp', pendingW + 'px'); grip.setAttribute('aria-valuenow', pendingW); });
    if (persist) save();
  };
  grip.addEventListener('pointerdown', (e) => {
    if (e.button !== 0) return;
    e.preventDefault();
    const scale = win.getBoundingClientRect().width / win.offsetWidth || 1;
    drag = { x: e.clientX, w: ui.w, scale };
    grip.setPointerCapture(e.pointerId); win.classList.add('sb-dragging');
  });
  grip.addEventListener('pointermove', (e) => { if (drag) setW(drag.w + (e.clientX - drag.x) / drag.scale, false); });
  const endDrag = () => { if (!drag) return; drag = null; win.classList.remove('sb-dragging'); save(); };
  grip.addEventListener('pointerup', endDrag);
  grip.addEventListener('pointercancel', endDrag);
  grip.addEventListener('lostpointercapture', endDrag);
  grip.addEventListener('dblclick', () => setW(SB_DEF, true));
  grip.addEventListener('keydown', (e) => {
    const d = e.key === 'ArrowRight' ? 10 : e.key === 'ArrowLeft' ? -10 : 0;
    if (d) { e.preventDefault(); setW(ui.w + d, true); }
    else if (e.key === 'Home') { e.preventDefault(); setW(SB_MIN, true); }
    else if (e.key === 'End') { e.preventDefault(); setW(SB_MAX, true); }
    else if (e.key === 'Enter') { e.preventDefault(); setW(SB_DEF, true); }
  });

  if (!Q) return;   // player.js failed: the rest needs its queue

  // ================= Up Next lists =================
  const ROW = 48, HINT_FADE = 48, HIST_MIN = 0.45;
  const ICON = (n) => `<svg class="ic" aria-hidden="true"><use href="#i-${n}"/></svg>`;
  const el = (tag, cls, text) => { const e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; };

  const rowsOf = (st) => {
    const out = [], seen = new Set();
    const key = (p, id) => { let k = p + id, n = 1; while (seen.has(k)) k = `${p}${id}#${n++}`; seen.add(k); return k; };
    const cur = st.current;
    const hist = st.history.filter((t) => !cur || t.id !== cur.id);
    if (hist.length) {
      out.push({ key: 'h-label', kind: 'hlabel' });
      hist.forEach((t, i) => out.push({ key: key('h-', t.id), kind: 'hist', t, d: hist.length - i }));
    }
    out.push({ key: 'l-now', kind: 'label', text: 'Now Playing' });
    out.push({ key: 's-' + cur.id, kind: 'cur', t: cur }); seen.add('s-' + cur.id);
    if (st.upcoming.length) {
      out.push({ key: 'l-next', kind: 'label', text: st.source ? `Continue playing from ${st.source}` : 'Up Next' });
      st.upcoming.forEach((t, j) => out.push({ key: key('s-', t.id), kind: 'up', t, j }));
    }
    if (st.lap.length) {
      out.push({ key: 'l-repeat', kind: 'repeat' });
      st.lap.forEach((t, k) => out.push({ key: key('k-', t.id), kind: 'lap', t, k }));
    }
    return { rows: out, hist: hist.length };
  };

  const makeNode = (r) => {
    const li = el('li');
    if (r.kind === 'label' || r.kind === 'hlabel' || r.kind === 'repeat') { li.className = 'un-label'; return li; }
    const main = el(r.kind === 'cur' ? 'div' : 'button', 'un-main');
    if (r.kind !== 'cur') main.type = 'button';
    const img = el('img', 'un-cover'); img.alt = ''; img.width = 34; img.height = 34; img.decoding = 'async'; img.src = r.t.cover;
    const text = el('div', 'un-text'); text.append(el('b'), el('span'));
    main.append(img, text, el('span', 'un-dur', '0:30'));
    li.append(main);
    if (r.kind === 'up') {
      const x = el('button', 'un-x'); x.type = 'button'; x.setAttribute('aria-label', 'Remove from Up Next'); x.title = 'Remove from Up Next'; x.innerHTML = ICON('close');
      li.append(x);
    }
    return li;
  };
  const updateNode = (li, r) => {
    li._r = r;
    li.className = { label: 'un-label', hlabel: 'un-label', repeat: 'un-label', cur: 'un-item un-cur', up: 'un-item', lap: 'un-item un-lap', hist: 'un-item un-hist' }[r.kind] || '';
    if (r.kind === 'hlabel') { li.innerHTML = ICON('clock') + '<span>History</span>'; return; }
    if (r.kind === 'repeat') { li.innerHTML = ICON('repeat') + '<span>Repeat</span>'; return; }
    if (r.kind === 'label') { let s = li.firstChild; if (!s || s.tagName !== 'SPAN') { li.textContent = ''; s = el('span'); li.append(s); } if (s.textContent !== r.text) s.textContent = r.text; return; }
    const main = li.firstChild, [b, a] = main.querySelector('.un-text').children;
    if (b.textContent !== r.t.title) b.textContent = r.t.title;
    const artist = r.t.artist; if (a.textContent !== artist) a.textContent = artist;
    const img = main.querySelector('img'); if (img.getAttribute('src') !== r.t.cover) img.src = r.t.cover;
    main.setAttribute('aria-label', r.kind === 'cur' ? `Now playing: ${r.t.title}, ${artist}` : `Play ${r.t.title}, ${artist}`);
    if (r.kind === 'cur') main.setAttribute('aria-current', 'true');
    if (r.kind === 'hist') li.style.setProperty('--ho', Math.max(HIST_MIN, 1 - 0.09 * r.d).toFixed(2));
  };

  // one list: a scroller with the rows, and the "History" pill. `look`: 'side' (right panel) or 'np'.
  const makeList = (wrap) => {
    wrap.classList.add('un-wrap');
    const scroll = el('div', 'un-scroll'), ul = el('ul', 'un-list'), pill = el('button', 'un-pill');
    pill.type = 'button'; pill.hidden = true;
    const msg = el('p', 'un-msg', 'Nothing is playing'); msg.hidden = true;
    ul.setAttribute('aria-label', 'Up Next');
    scroll.append(ul); wrap.append(scroll, pill, msg);
    const nodes = new Map();
    let hiddenH = 0, nHist = 0, raf = 0;

    const onScroll = () => {
      raf = 0;
      const t = scroll.scrollTop;
      scroll.classList.toggle('fade-t', t > 1);
      scroll.classList.toggle('fade-b', t + scroll.clientHeight < scroll.scrollHeight - 1);
      if (!nHist) { pill.hidden = true; return; }
      pill.hidden = false;
      const shown = clamp((hiddenH - t) / HINT_FADE, 0, 1);
      pill.style.opacity = (1 - shown).toFixed(2);
      pill.style.pointerEvents = shown >= 1 ? 'none' : '';
      pill.style.visibility = shown >= 1 ? 'hidden' : '';
      pill.style.transform = `translate(-50%, ${(-6 * shown).toFixed(1)}px)`;
    };
    scroll.addEventListener('scroll', () => { if (!raf) raf = requestAnimationFrame(onScroll); }, { passive: true });

    pill.addEventListener('click', () => {
      const top = Math.max(hiddenH - (scroll.clientHeight - 2 * ROW - 16), 0);
      scroll.scrollTo({ top, behavior: reduced.matches ? 'auto' : 'smooth' });
    });
    ul.addEventListener('click', (e) => {
      const li = e.target.closest('li'); const r = li && li._r; if (!r) return;
      if (e.target.closest('.un-x')) { Q.remove(r.j); return; }
      if (!e.target.closest('button.un-main')) return;
      if (r.kind === 'up') Q.jump(r.j); else if (r.kind === 'lap') Q.jumpLap(r.k); else if (r.kind === 'hist') Q.playHistory(r.t.id);
    });

    // keep Now Playing where it is when History grows above it (the app's scroll anchor)
    const place = () => { scroll.scrollTop = hiddenH; onScroll(); };

    const render = (animate) => {
      const st = Q.state();
      const empty = !st.started || !st.current;
      msg.hidden = !empty; scroll.hidden = empty;
      if (empty) { pill.hidden = true; return; }
      const { rows, hist } = rowsOf(st);
      const wasAtNow = Math.abs(scroll.scrollTop - hiddenH) < 3;
      const focused = ul.contains(document.activeElement) ? document.activeElement : null;
      const before = new Map();
      if (animate) nodes.forEach((n, k) => before.set(k, n.offsetTop - scroll.scrollTop));

      const keep = new Set(rows.map((r) => r.key));
      nodes.forEach((n, k) => { if (!keep.has(k)) { n.remove(); nodes.delete(k); } });
      let prev = null;
      for (const r of rows) {
        let n = nodes.get(r.key);
        if (n && n._r.kind !== r.kind) { const m = makeNode(r); n.replaceWith(m); n = m; nodes.set(r.key, n); }   // e.g. an upcoming row that became Now Playing: same song, new shape
        const fresh = !n && !before.has(r.key);
        if (!n) { n = makeNode(r); nodes.set(r.key, n); }
        updateNode(n, r);
        if (fresh && animate) n.classList.add('un-enter');
        const want = prev ? prev.nextSibling : ul.firstChild;
        if (n !== want) ul.insertBefore(n, want);
        prev = n;
      }
      if (focused && !focused.isConnected) {   // its row went (removed or played): keep the keyboard where it was
        const f = ul.querySelector('.un-cur .un-main'); if (f) f.focus({ preventScroll: true });
      } else if (focused && document.activeElement !== focused) focused.focus({ preventScroll: true });

      hiddenH = hist ? (hist + 1) * ROW : 0; nHist = hist;
      ul.style.minHeight = (hiddenH + scroll.clientHeight) + 'px';   // room to bring Now Playing to the top, as the app's blank rows do
      if (!animate) { nodes.forEach((n) => n.classList.remove('un-enter')); return; }
      if (wasAtNow) scroll.scrollTop = hiddenH;
      onScroll();

      // FLIP: rows that moved start at their old place and ease to the new one (transform only); new rows fade in
      const moved = [], entering = [];
      nodes.forEach((n, k) => {
        if (!before.has(k)) { entering.push(n); return; }
        const d = before.get(k) - (n.offsetTop - scroll.scrollTop);
        if (d) moved.push([n, d]);
      });
      if (reduced.matches || moved.length > 14) { nodes.forEach((n) => n.classList.remove('un-enter')); return; }
      moved.forEach(([n, d]) => { n.style.transition = 'none'; n.style.transform = `translate3d(0,${d}px,0)`; });
      void ul.offsetHeight;
      requestAnimationFrame(() => {
        moved.forEach(([n]) => {
          n.style.transition = 'transform .3s cubic-bezier(.2,.8,.2,1)'; n.style.transform = '';
          const done = () => { n.style.transition = ''; n.removeEventListener('transitionend', done); };
          n.addEventListener('transitionend', done);
        });
        entering.forEach((n) => n.classList.add('un-in'));
        setTimeout(() => entering.forEach((n) => n.classList.remove('un-enter', 'un-in')), 400);
      });
    };
    return { wrap, render, place, onScroll };
  };

  // ================= right panel =================
  const panel = $('dw-rp'), paneL = $('rp-lyrics'), paneQ = $('rp-queue'), lyrMsg = $('rp-lyrics-msg');
  const tabs = { lyrics: $('rp-tab-lyrics'), queue: $('rp-tab-queue') };
  const bar = { lyrics: $('dw-lyrics'), queue: $('dw-queue') };
  const sideList = makeList(paneQ);
  let rp = { open: false, tab: 'lyrics' };   // the app's defaults: closed, Lyrics

  const syncPanel = (placeList) => {
    const q = rp.tab === 'queue';
    for (const k of ['lyrics', 'queue']) {
      const on = rp.tab === k;
      tabs[k].setAttribute('aria-selected', String(on)); tabs[k].tabIndex = on ? 0 : -1;
      bar[k].setAttribute('aria-pressed', String(rp.open && on));
    }
    paneL.hidden = q; paneQ.hidden = !q;
    win.classList.toggle('rp-open', rp.open);
    panel.inert = !rp.open;
    lyrMsg.textContent = Q.state().started ? 'No lyrics for this song' : 'Nothing is playing';   // the demo songs have none, as the app says
    if (rp.open && q) { sideList.render(false); if (placeList) sideList.place(); }
  };
  const openPanel = (tab, focusIn) => {
    rp = { open: true, tab };
    syncPanel(true);
    if (focusIn) tabs[tab].focus({ preventScroll: true });
  };
  const closePanel = (focusBack) => {
    if (!rp.open) return;
    const inside = panel.contains(document.activeElement);
    const tab = rp.tab;
    rp.open = false; syncPanel(false);
    if (focusBack || inside) bar[tab].focus({ preventScroll: true });
  };
  // the player's buttons (app: show_right_tab): the same tab again hides the panel
  const showTab = (tab, e) => {
    if (rp.open && rp.tab === tab) closePanel(false);
    else openPanel(tab, e && e.detail === 0);
  };
  bar.lyrics.addEventListener('click', (e) => showTab('lyrics', e));
  bar.queue.addEventListener('click', (e) => showTab('queue', e));
  // the switch never closes the panel
  for (const k of ['lyrics', 'queue']) tabs[k].addEventListener('click', () => { if (rp.tab !== k) { rp.tab = k; syncPanel(true); } });
  panel.querySelector('.rp-switch').addEventListener('keydown', (e) => {
    const d = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0;
    if (!d && e.key !== 'Home' && e.key !== 'End') return;
    e.preventDefault();
    const k = e.key === 'Home' ? 'lyrics' : e.key === 'End' ? 'queue' : (rp.tab === 'lyrics') === (d > 0) ? 'queue' : 'lyrics';
    rp.tab = k; syncPanel(true); tabs[k].focus({ preventScroll: true });
  });
  $('rp-close').addEventListener('click', () => closePanel(true));

  // Escape: closes the panel unless something above it (volume popover, effect picker, Now Playing) wants the key first
  const npEl = $('dw-np'), volPop = $('dw-pop');
  document.addEventListener('keydown', (e) => {
    if (e.key !== 'Escape' || !rp.open || e.defaultPrevented) return;
    const t = e.target;
    if (!(win.contains(t) || t === document.body || t === document.documentElement)) return;
    const fx = $('dw-fxpop');
    if ((fx && !fx.hidden) || !volPop.hidden || npEl.classList.contains('open')) return;
    e.preventDefault(); closePanel(false);
  }, true);

  // ================= Now Playing: Up Next column (app: the queue button, taking turns with the lyrics) =================
  const npBody = win.querySelector('.np-body'), npRight = $('np-right'), npLyr = $('np-lyrics'), npQ = $('np-queue'), npNoLyr = $('np-nolyrics');
  const npList = makeList(npRight);
  let np = { lyrics: true, queue: false };
  const npOpen = () => npEl.classList.contains('open');
  const syncNp = (placeList) => {
    const lyricsOn = np.lyrics && !np.queue;
    npLyr.classList.toggle('on', lyricsOn); npLyr.setAttribute('aria-pressed', String(lyricsOn)); npLyr.setAttribute('aria-label', lyricsOn ? 'Hide lyrics' : 'Show lyrics');
    npQ.classList.toggle('on', np.queue); npQ.setAttribute('aria-pressed', String(np.queue)); npQ.setAttribute('aria-label', np.queue ? 'Hide Up Next' : 'Show Up Next');
    npBody.classList.toggle('side', np.queue); npRight.hidden = !np.queue;
    npNoLyr.hidden = !lyricsOn;   // the demo has no lyrics: the app shows this note for songs without any
    if (np.queue && npOpen()) { npList.render(false); if (placeList) npList.place(); }
  };
  npLyr.addEventListener('click', () => { if (np.queue) { np.queue = false; np.lyrics = true; } else np.lyrics = !np.lyrics; syncNp(false); });
  npQ.addEventListener('click', () => { np.queue = !np.queue; syncNp(true); });
  let npWasOpen = false;
  new MutationObserver(() => { const o = npOpen(); if (o !== npWasOpen) { npWasOpen = o; if (o) syncNp(true); } }).observe(npEl, { attributes: true, attributeFilter: ['class'] });
  syncNp(false);

  // ================= model changes =================
  Q.on(() => {
    if (rp.open && rp.tab === 'queue') sideList.render(true);
    if (rp.tab === 'lyrics') lyrMsg.textContent = 'No lyrics for this song';
    if (np.queue && npOpen()) npList.render(true);
    // opening a panel before anything played shows "Nothing is playing"; the first song fills it in
  });
  syncPanel(false);
})();
