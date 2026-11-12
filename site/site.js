/* Uzumaki site: skip the startup intro on a click, key or scroll, and fade sections in as they
   scroll into view with ONE IntersectionObserver. The intro itself is pure CSS (style.css). */
(() => {
  const root = document.documentElement;
  root.classList.add('rv');                       // site.js ran: lifts the .js-fail safety net

  if (root.classList.contains('intro-on')) {
    const skip = () => {
      root.classList.add('intro-done');
      for (const t of ['pointerdown', 'keydown', 'wheel', 'touchstart', 'scroll']) removeEventListener(t, skip);
    };
    for (const t of ['pointerdown', 'keydown', 'wheel', 'touchstart', 'scroll']) addEventListener(t, skip, { passive: true });
  }

  const items = document.querySelectorAll('.reveal');
  if (!items.length) return;
  if (!('IntersectionObserver' in window) || matchMedia('(prefers-reduced-motion: reduce)').matches) {
    for (const el of items) el.classList.add('in');
    return;
  }
  const io = new IntersectionObserver((entries) => {
    for (const e of entries) {
      if (!e.isIntersecting) continue;
      e.target.classList.add('in');
      io.unobserve(e.target);
    }
  }, { rootMargin: '0px 0px -8% 0px', threshold: 0.08 });
  for (const el of items) io.observe(el);
})();
