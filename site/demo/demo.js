/* Uzumaki demo, stage 1: fit the window to the viewport and tilt/flatten it while the hero scrolls.
   Only transform + opacity change per frame; measurements happen on resize only. */
(() => {
  const hero = document.getElementById('top'), sticky = document.getElementById('demo-sticky');
  const text = document.getElementById('demo-text'), stage = document.getElementById('demo-stage');
  const tilt = document.getElementById('demo-tilt');
  if (!hero || !sticky || !stage || !tilt) return;
  const W = 1100, H = 690;
  const mqSmall = matchMedia('(max-width: 899px)'), mqReduce = matchMedia('(prefers-reduced-motion: reduce)');
  const clamp = (v, a, b) => Math.min(b, Math.max(a, v));
  let flat = true, travel = 1, startY = 0, ticking = false;

  const measure = () => {
    flat = mqSmall.matches || mqReduce.matches;
    hero.classList.toggle('demo-flat', flat);
    const vw = document.documentElement.clientWidth, vh = innerHeight;
    // fit: width always; on the tall hero also keep the flattened window inside the viewport height
    let fit = Math.min(1, (vw - 40) / W);
    if (!flat) fit = Math.min(fit, (vh - 96) / H);
    hero.style.setProperty('--fit', fit.toFixed(4));
    if (flat) { hero.style.height = ''; sticky.style.height = ''; tilt.style.transform = ''; text.style.cssText = ''; stage.style.cssText = ''; return; }
    const navH = 64, winH = H * fit;
    sticky.style.height = vh + 'px';
    sticky.style.top = '0px';
    hero.style.height = Math.round(vh * 2.15) + 'px';
    travel = hero.offsetHeight - vh;
    // start: window sits just under the hero text; end: window centred in the viewport below the nav
    const endTop = navH + (vh - navH - winH) / 2;
    const textBottom = text.offsetTop + text.offsetHeight;
    const startTop = Math.max(endTop, textBottom + 24);
    startY = startTop - endTop;
    stage.style.marginTop = '0px';
    stage.style.position = 'absolute'; stage.style.left = '50%'; stage.style.top = endTop + 'px'; stage.style.marginLeft = (-W * fit / 2) + 'px';
    frame();
  };

  const frame = () => {
    ticking = false;
    if (flat) return;
    const p = clamp((scrollY - hero.offsetTop) / travel, 0, 1);
    const e = p * p * (3 - 2 * p);                  // smoothstep
    const t = clamp(p / .7, 0, 1);                  // flatten a bit before the end, then hold
    const k = t * t * (3 - 2 * t);
    tilt.style.transform = `translate3d(0,${(startY * (1 - k)).toFixed(1)}px,0) rotateX(${(16 * (1 - k)).toFixed(2)}deg) scale(${(.85 + .15 * k).toFixed(4)})`;
    text.style.transform = `translate3d(0,${(-e * 90).toFixed(1)}px,0)`;
    text.style.opacity = clamp(1 - p * 2.4, 0, 1).toFixed(3);
    text.style.pointerEvents = p > .4 ? 'none' : '';
  };
  const onScroll = () => { if (!ticking) { ticking = true; requestAnimationFrame(frame); } };

  addEventListener('scroll', onScroll, { passive: true });
  addEventListener('resize', measure);
  mqSmall.addEventListener('change', measure); mqReduce.addEventListener('change', measure);
  // current cover feeds the wallpaper (hook for the WASM backgrounds later)
  const wall = document.getElementById('dw-wall'), cover = document.getElementById('dw-cover');
  const first = document.querySelector('.dw-tile img');
  const firstTile = document.querySelector('.dw-tile'); if (firstTile) firstTile.classList.add('on');
  if (first && cover) { cover.src = first.src; wall.style.setProperty('--wall', `url("${first.src}")`); }
  if (document.fonts && document.fonts.ready) document.fonts.ready.then(measure);
  measure();
})();
