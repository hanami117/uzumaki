/* Uzumaki demo: renders the app's real background effects (crate uzumaki-fx, compiled to fx.wasm:
   built from fx-web/) off the main thread.
   in:  {slot, id, url, w, h, unit, effect, bitmap?}   (newest job per slot wins; older queued ones are dropped).
        `url` is the picture's address and its cache key; with `bitmap` (an ImageBitmap the page already decoded: the
        "Make it yours" preview's own picture) nothing is fetched and `url` is only a key.
   out: {ready:true} | {error, fatal?} | {slot, id, url, w, h, bitmap | rgba, ms:{fetch, decode, render}} */
'use strict';
let ex = null;
const covers = new Map();           // url -> {w, h, px: Uint8ClampedArray}, newest last, at most 4
const pending = new Map();          // slot -> job
let busy = false;

const init = (async () => {
  if (typeof WebAssembly !== 'object' || typeof OffscreenCanvas === 'undefined' || typeof createImageBitmap !== 'function') throw new Error('unsupported');
  const url = new URL('fx.wasm', self.location.href);
  let inst;
  try { inst = await WebAssembly.instantiateStreaming(fetch(url), {}); }
  catch (_) { inst = await WebAssembly.instantiate(await (await fetch(url)).arrayBuffer(), {}); }
  ex = inst.instance.exports;
})();

async function cover(job, ms) {
  const url = job.url;
  let c = covers.get(url);
  if (c) { covers.delete(url); covers.set(url, c); return c; }
  let t = performance.now(), bmp = job.bitmap;
  if (!bmp) {
    const res = await fetch(url, { mode: 'cors', credentials: 'omit' });
    if (!res.ok) throw new Error('cover ' + res.status);
    const blob = await res.blob();
    ms.fetch = performance.now() - t; t = performance.now();
    // 'none': raw pixel values, like the app's decoder (no colour-profile conversion)
    bmp = await createImageBitmap(blob, { colorSpaceConversion: 'none', premultiplyAlpha: 'none' });
  }
  const cv = new OffscreenCanvas(bmp.width, bmp.height), cx = cv.getContext('2d', { willReadFrequently: true });
  cx.drawImage(bmp, 0, 0);
  const d = cx.getImageData(0, 0, bmp.width, bmp.height);
  bmp.close();
  ms.decode = performance.now() - t;
  c = { w: d.width, h: d.height, px: d.data };
  covers.set(url, c);
  if (covers.size > 4) covers.delete(covers.keys().next().value);
  return c;
}

async function run(job) {
  const ms = { fetch: 0, decode: 0, render: 0 };
  const c = await cover(job, ms);
  const t = performance.now();
  const src = ex.fx_alloc(c.px.length);
  new Uint8Array(ex.memory.buffer, src, c.px.length).set(c.px);   // fx_render takes ownership of `src`
  const out = ex.fx_render(src, c.w, c.h, job.w, job.h, job.unit, job.effect, 0);
  if (!out) throw new Error('render failed');
  const ow = ex.fx_out_width(), oh = ex.fx_out_height(), n = ow * oh * 4;
  ms.render = performance.now() - t;
  const view = new Uint8ClampedArray(ex.memory.buffer, out, n);
  const msg = { slot: job.slot, id: job.id, url: job.url, w: ow, h: oh, ms };
  let transfer;
  try {
    msg.bitmap = await createImageBitmap(new ImageData(view, ow, oh), { colorSpaceConversion: 'none', premultiplyAlpha: 'none' });
    transfer = [msg.bitmap];
  } catch (_) {
    msg.rgba = view.slice().buffer; transfer = [msg.rgba];
  } finally { ex.fx_free(out, n); }
  postMessage(msg, transfer);
}

async function pump() {
  if (busy) return;
  busy = true;
  try {
    await init;
    while (pending.size) {
      const [slot, job] = pending.entries().next().value;
      pending.delete(slot);
      try { await run(job); }
      catch (e) {
        // a wasm trap leaves the instance unusable: tell the page to fall back for good
        const fatal = e instanceof WebAssembly.RuntimeError;
        postMessage({ slot: job.slot, id: job.id, error: String(e && e.message || e), fatal });
      }
    }
  } catch (e) { postMessage({ error: String(e && e.message || e), fatal: true }); }
  busy = false;
}

self.onmessage = (e) => { const j = e.data; pending.set(j.slot, j); pump(); };
init.then(() => postMessage({ ready: true }), (e) => postMessage({ error: String(e && e.message || e), fatal: true }));
