// node --test bridge/   (no Electron, no network)
'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { applyStorefront } = require('./signin.js');

// MusicKit's storefrontId: the page's (URL) storefront if set, else the account's.
const fakeMk = ({ page = 'us', account = 'it', authorized = true, previewOnly = true } = {}) => {
  let api = page;
  return {
    isAuthorized: authorized,
    storefrontCountryCode: account,
    previewOnly,
    sets: 0,
    get storefrontId() {
      return api || this.storefrontCountryCode;
    },
    set storefrontId(v) {
      this.sets++;
      api = v;
    },
  };
};

test('a US page signed in with an Italian account switches to it and plays full songs', () => {
  const mk = fakeMk();
  const r = applyStorefront(mk, 'IT');
  assert.deepEqual(r, { before: 'us', storefront: 'it', account: 'it', previewOnly: false });
  assert.equal(mk.sets, 1);
});

test('already right: nothing is set', () => {
  const mk = fakeMk({ page: 'it', previewOnly: false });
  assert.deepEqual(applyStorefront(mk, 'it'), { before: 'it', storefront: 'it', account: 'it', previewOnly: false });
  assert.equal(mk.sets, 0);
});

test('preview-only stays when MusicKit does not know the account is there (or signed out)', () => {
  const stale = fakeMk({ account: 'us' });
  assert.equal(applyStorefront(stale, 'it').previewOnly, true);
  const out = fakeMk({ authorized: false });
  assert.equal(applyStorefront(out, 'it').previewOnly, true);
  // Never turned on.
  const off = fakeMk({ account: 'us', previewOnly: false });
  assert.equal(applyStorefront(off, 'it').previewOnly, false);
});

test('a bad id is refused and changes nothing', () => {
  for (const bad of [undefined, null, '', 'ita', '1x', 'u s']) {
    const mk = fakeMk();
    assert.throws(() => applyStorefront(mk, bad));
    assert.equal(mk.storefrontId, 'us');
  }
});
