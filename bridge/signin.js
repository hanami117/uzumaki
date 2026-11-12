// The guided sign-in's window: its size and place, the `window` event, and the light CSS layer
// around Apple's sign-in dialog. Plain functions with
// no Electron in them, so `node --test bridge/signin.test.js` checks them.
//
// The CSS layer is the only thing here that styles music.apple.com. To remove it, delete
// SIGN_IN_CSS and the `cadence:signin-css` handler in preload.js. It never styles Apple's dialog
// (a cross-origin frame, which no CSS from this page can reach anyway) and never reads anything
// from it. It fails open: while no sign-in frame is found, nothing is marked and the page shows
// as it is.
'use strict';

// About the size of Apple's sign-in dialog plus its margins.
const SIGN_IN_SIZE = { width: 520, height: 740 };
// The full window (`show`, for debugging).
const FULL_SIZE = { width: 1100, height: 800 };
// Our window's background (theme.rs PAL.bg, dark), shown while the page loads and around the dialog.
const SIGN_IN_BG = '#0e0e11';
const SIGN_IN_TITLE = 'Sign in to Apple Music';
const FULL_TITLE = 'Uzumaki: sign in to Apple Music';

const finite = (n) => typeof n === 'number' && Number.isFinite(n);

// A usable `near` rectangle ({x, y, w, h} in screen points), or null.
function nearRect(near) {
  if (!near || ![near.x, near.y, near.w, near.h].every(finite) || near.w <= 0 || near.h <= 0) return null;
  return { x: near.x, y: near.y, width: near.w, height: near.h };
}

// Where the sign-in window goes: SIGN_IN_SIZE (smaller on a small screen), centred on `near` (our
// window's frame, macOS) when given, else on `workArea` (the screen), and kept inside `workArea`.
function signInBounds(near, workArea, size = SIGN_IN_SIZE) {
  const wa = workArea;
  const width = Math.max(320, Math.min(size.width, wa.width - 24));
  const height = Math.max(400, Math.min(size.height, wa.height - 24));
  const c = nearRect(near) || wa;
  let x = Math.round(c.x + (c.width - width) / 2);
  let y = Math.round(c.y + (c.height - height) / 2);
  x = Math.max(wa.x, Math.min(x, wa.x + wa.width - width));
  y = Math.max(wa.y, Math.min(y, wa.y + wa.height - height));
  return { x, y, width, height };
}

// The `window` event. `reason` on a hide: 'closed' (the user closed the window) or 'hidden' (the
// app asked: `hide`, or signed in). Old apps ignore it.
function windowEvent(visible, reason) {
  const ev = { event: 'window', visible: !!visible };
  if (!visible) ev.reason = reason === 'closed' ? 'closed' : 'hidden';
  return ev;
}

// Hosts whose frame is Apple's sign-in. Only the frame's `src` is looked at.
const SIGN_IN_HOSTS = ['idmsa.apple.com', 'appleid.apple.com', 'authorize.music.apple.com'];

function isSignInFrame(src) {
  try {
    const host = new URL(src).hostname;
    return SIGN_IN_HOSTS.some((h) => host === h || host.endsWith('.' + h));
  } catch {
    return false;
  }
}

// Set by preload.js on music.apple.com's own elements only: `data-uzu-signin` on <html> while the
// dialog is found, `data-uzu-path` on the dialog's ancestors, `data-uzu-dialog` on the dialog's
// box. Everything beside that path fades to our background; the dialog and all inside it are left
// alone.
const SIGN_IN_CSS = `
html[data-uzu-signin], html[data-uzu-signin] body { background: ${SIGN_IN_BG} !important; }
html[data-uzu-signin] [data-uzu-path] { background: transparent !important; }
html[data-uzu-signin] [data-uzu-path] > :not([data-uzu-path]):not([data-uzu-dialog]) {
  opacity: 0 !important; pointer-events: none !important; transition: opacity .2s ease-out;
}
`;

module.exports = {
  SIGN_IN_SIZE, FULL_SIZE, SIGN_IN_BG, SIGN_IN_TITLE, FULL_TITLE, SIGN_IN_HOSTS, SIGN_IN_CSS,
  signInBounds, windowEvent, isSignInFrame,
};
