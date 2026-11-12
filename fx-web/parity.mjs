// Wasm half of the parity check (see examples/parity.rs): node fx-web/parity.mjs path/to/fx.wasm
import { readFileSync } from 'node:fs';
const { instance } = await WebAssembly.instantiate(readFileSync(process.argv[2]), {});
const x = instance.exports;
const sw = 600, sh = 600, src = new Uint8Array(sw * sh * 4);
for (let y = 0; y < sh; y++) for (let px = 0; px < sw; px++) {
  const o = (y * sw + px) * 4;
  src[o] = Math.floor(px * 255 / 600); src[o + 1] = Math.floor(y * 255 / 600); src[o + 2] = (px ^ y) & 255; src[o + 3] = 255;
}
for (let e = 0; e < x.fx_effect_count(); e++) for (const [w, h, unit] of [[1360, 860, 1], [700, 500, 2]]) {
  const p = x.fx_alloc(src.length);
  new Uint8Array(x.memory.buffer, p, src.length).set(src);
  const t0 = performance.now();
  const out = x.fx_render(p, sw, sh, w, h, unit, e, 0);
  const ms = performance.now() - t0;
  const ow = x.fx_out_width(), oh = x.fx_out_height(), n = ow * oh * 4;
  const bytes = new Uint8Array(x.memory.buffer, out, n);
  let hash = 0xcbf29ce484222325n;
  for (let i = 0; i < n; i++) hash = ((hash ^ BigInt(bytes[i])) * 0x100000001b3n) & 0xffffffffffffffffn;
  console.log(`${e} ${w}x${h}@${unit} -> ${ow}x${oh} ${hash.toString(16).padStart(16, '0')}`);
  console.error(`  ${e} ${w}x${h}: ${ms.toFixed(0)} ms`);
  x.fx_free(out, n);
}
