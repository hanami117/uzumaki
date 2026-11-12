// node --test bridge/   (no Electron, no network)
'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { signInBounds, windowEvent, isSignInFrame, SIGN_IN_SIZE, SIGN_IN_CSS } = require('./signin.js');

const screen = { x: 0, y: 25, width: 1512, height: 920 }; // a MacBook's work area below the menu bar

test('centred on our window when it is given', () => {
  const near = { x: 100, y: 100, w: 1360, h: 860 };
  const b = signInBounds(near, screen);
  assert.equal(b.width, SIGN_IN_SIZE.width);
  assert.equal(b.height, SIGN_IN_SIZE.height);
  assert.equal(b.x + b.width / 2, 100 + 1360 / 2);
  assert.equal(b.y + b.height / 2, 100 + 860 / 2);
});

test('centred on the screen without our window (Linux)', () => {
  for (const near of [undefined, null, {}, { x: 0, y: 0, w: 0, h: 0 }, { x: 'a', y: 1, w: 2, h: 3 }]) {
    const b = signInBounds(near, screen);
    assert.equal(b.x, Math.round((1512 - 520) / 2));
    assert.equal(b.y, 25 + Math.round((920 - 740) / 2));
  }
});

test('kept on the screen when our window hangs off its edge', () => {
  const b = signInBounds({ x: -900, y: 600, w: 1000, h: 800 }, screen);
  assert.equal(b.x, 0);
  assert.equal(b.y, 25 + 920 - 740);
});

test('smaller on a small screen', () => {
  const b = signInBounds(null, { x: 0, y: 0, width: 1366, height: 700 });
  assert.equal(b.height, 676);
  assert.equal(b.y, 12);
});

test('window event: a reason only on a hide', () => {
  assert.deepEqual(windowEvent(true), { event: 'window', visible: true });
  assert.deepEqual(windowEvent(false, 'closed'), { event: 'window', visible: false, reason: 'closed' });
  assert.deepEqual(windowEvent(false, 'hidden'), { event: 'window', visible: false, reason: 'hidden' });
  assert.deepEqual(windowEvent(false), { event: 'window', visible: false, reason: 'hidden' });
});

test('only Apple sign-in frames count', () => {
  assert.ok(isSignInFrame('https://idmsa.apple.com/appleauth/auth/authorize/signin?frame_id=x'));
  assert.ok(isSignInFrame('https://authorize.music.apple.com/woa'));
  assert.ok(!isSignInFrame('https://music.apple.com/us/browse'));
  assert.ok(!isSignInFrame('https://idmsa.apple.com.evil.example/'));
  assert.ok(!isSignInFrame('about:blank'));
  assert.ok(!isSignInFrame(''));
});

test('the CSS layer never names the dialog or a frame', () => {
  assert.ok(!/iframe/i.test(SIGN_IN_CSS));
  assert.ok(SIGN_IN_CSS.includes(':not([data-uzu-dialog])'));
});
