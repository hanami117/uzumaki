import { test } from "node:test";
import assert from "node:assert/strict";

import { bucket, day, handle, normalize } from "../src/index.js";

const LATEST = {
  version: "0.2.0",
  build: 1,
  date: "2026-12-01",
  notes_url: "https://github.com/hanami117/uzumaki/releases/tag/v0.2.0",
  downloads: { macos_dmg: "https://example.invalid/Uzumaki-macOS.dmg" },
};
const NOW = new Date("2026-11-12T23:59:59Z");

/** A D1 stand-in that records the upserts and answers the stats query with `rows`. */
function fakeDb(rows = []) {
  const runs = [];
  const queries = [];
  return {
    runs,
    queries,
    prepare(sql) {
      return {
        bind(...args) {
          return {
            run: async () => runs.push({ sql, args }),
            all: async () => {
              queries.push({ sql, args });
              return { results: rows };
            },
          };
        },
      };
    },
  };
}

function fakeCtx() {
  const pending = [];
  return { pending, waitUntil: (p) => pending.push(p), settle: () => Promise.all(pending) };
}

function fetcherFor(body, status = 200) {
  const calls = [];
  const fetcher = async (url, init) => {
    calls.push({ url, init });
    if (body instanceof Error) throw body;
    return new Response(typeof body === "string" ? body : JSON.stringify(body), { status });
  };
  fetcher.calls = calls;
  return fetcher;
}

async function run(path, { env = {}, method = "GET", headers = {}, source = LATEST } = {}) {
  const db = env.DB ?? fakeDb();
  const ctx = fakeCtx();
  const fetcher = fetcherFor(source);
  const resp = await handle(new Request(`https://updates.getuzumaki.app${path}`, { method, headers }), { DB: db, ...env }, ctx, {
    now: NOW,
    fetcher,
  });
  await ctx.settle();
  const text = await resp.text();
  return { resp, body: text ? JSON.parse(text) : null, db, fetcher };
}

const q = (s) => new URLSearchParams(s);

test("bucket: a full, valid check", () => {
  assert.deepEqual(bucket(q("v=0.2.0&os=linux&arch=x86_64&ch=appimage")), {
    version: "0.2.0",
    os: "linux",
    arch: "x86_64",
    channel: "appimage",
  });
});

test("bucket: a leading v and a pre-release are fine", () => {
  assert.equal(bucket(q("v=v0.3.0-beta.2&os=macos")).version, "0.3.0-beta.2");
});

test("bucket: unknown arch and channel count as unknown", () => {
  const b = bucket(q("v=0.2.0&os=macos&arch=sparc&ch=whatever"));
  assert.equal(b.arch, "unknown");
  assert.equal(b.channel, "unknown");
  assert.equal(bucket(q("v=0.2.0&os=macos")).channel, "unknown");
});

test("bucket: no readable version or OS is not counted", () => {
  for (const s of ["", "os=linux", "v=0.2.0", "v=banana&os=linux", "v=0.2&os=linux", "v=0.2.0&os=haiku", "v=0.2.0.1&os=linux"]) {
    assert.equal(bucket(q(s)), null, s);
  }
  assert.equal(bucket(q(`v=0.2.0-${"x".repeat(200)}&os=linux`)), null, "no unbounded strings");
});

test("bucket: repeat=1 is not counted", () => {
  assert.equal(bucket(q("v=0.2.0&os=linux&repeat=1")), null);
  assert.ok(bucket(q("v=0.2.0&os=linux&repeat=0")));
});

test("day is the UTC date", () => {
  assert.equal(day(NOW), "2026-11-12");
  assert.equal(day(new Date("2026-11-13T00:00:00Z")), "2026-11-13");
});

test("normalize: minimum_supported is always present", () => {
  assert.equal(normalize(LATEST).minimum_supported, null);
  assert.equal(normalize({ ...LATEST, minimum_supported: "0.1.2" }).minimum_supported, "0.1.2");
  assert.equal(normalize({ ...LATEST, minimum_supported: "nope" }).minimum_supported, null);
  assert.equal(normalize({ ...LATEST, minimum_supported: 3 }).minimum_supported, null);
});

test("normalize: an unusable file is null", () => {
  for (const bad of [null, [], "0.2.0", {}, { version: "x" }, { version: 2 }]) {
    assert.equal(normalize(bad), null, JSON.stringify(bad));
  }
});

test("GET /latest.json answers the site's file and counts once", async () => {
  const { resp, body, db, fetcher } = await run("/latest.json?v=0.1.0&os=macos&arch=aarch64&ch=homebrew");
  assert.equal(resp.status, 200);
  assert.equal(resp.headers.get("cache-control"), "no-store");
  assert.match(resp.headers.get("content-type"), /application\/json/);
  assert.deepEqual(body, { ...LATEST, minimum_supported: null });
  assert.equal(fetcher.calls[0].url, "https://getuzumaki.app/latest.json");
  assert.ok(fetcher.calls[0].init.cf.cacheTtl > 0, "the source is cached");
  assert.equal(db.runs.length, 1);
  assert.deepEqual(db.runs[0].args, ["2026-11-12", "0.1.0", "macos", "aarch64", "homebrew"]);
  assert.match(db.runs[0].sql, /ON CONFLICT .* DO UPDATE SET n = n \+ 1/);
});

test("nothing but the bucket reaches the database", async () => {
  const { db } = await run("/latest.json?v=0.1.0&os=linux&arch=x86_64&ch=aur&id=abc123&email=a@b.c", {
    headers: { "user-agent": "Uzumaki/0.1.0", "cf-connecting-ip": "203.0.113.7", cookie: "x=1" },
  });
  const stored = JSON.stringify(db.runs);
  for (const leak of ["abc123", "a@b.c", "203.0.113.7", "Uzumaki/", "x=1"]) {
    assert.ok(!stored.includes(leak), leak);
  }
});

test("LATEST_SOURCE overrides where latest.json comes from", async () => {
  const { fetcher } = await run("/latest.json", { env: { LATEST_SOURCE: "https://uzumaki.pages.dev/latest.json" } });
  assert.equal(fetcher.calls[0].url, "https://uzumaki.pages.dev/latest.json");
});

test("a check without parameters (a browser) is answered, not counted", async () => {
  const { resp, db } = await run("/latest.json");
  assert.equal(resp.status, 200);
  assert.equal(db.runs.length, 0);
});

test("HEAD is answered without a body and not counted", async () => {
  const { resp, body, db } = await run("/latest.json?v=0.1.0&os=linux", { method: "HEAD" });
  assert.equal(resp.status, 200);
  assert.equal(body, null);
  assert.equal(db.runs.length, 0);
});

test("the source failing is a 502, and the check still counts", async () => {
  for (const source of [new Error("down"), "not json", { version: "garbage" }]) {
    const { resp, db } = await run("/latest.json?v=0.1.0&os=linux", { source });
    assert.equal(resp.status, 502);
    assert.equal(resp.headers.get("cache-control"), "no-store");
    assert.equal(db.runs.length, 1);
  }
});

test("a failing counter never fails the check", async () => {
  const DB = { prepare: () => ({ bind: () => ({ run: async () => { throw new Error("D1 down"); } }) }) };
  const { resp, body } = await run("/latest.json?v=0.1.0&os=linux", { env: { DB } });
  assert.equal(resp.status, 200);
  assert.equal(body.version, "0.2.0");
});

test("other paths are 404, other methods 405", async () => {
  assert.equal((await run("/")).resp.status, 404);
  assert.equal((await run("/latest")).resp.status, 404);
  const post = await run("/latest.json?v=0.1.0&os=linux", { method: "POST" });
  assert.equal(post.resp.status, 405);
  assert.equal(post.db.runs.length, 0);
});

test("/stats does not exist without STATS_TOKEN", async () => {
  const { resp, db } = await run("/stats", { headers: { authorization: "Bearer anything" } });
  assert.equal(resp.status, 404);
  assert.equal(db.queries.length, 0);
});

test("/stats wants the token", async () => {
  const env = { STATS_TOKEN: "s3cret" };
  for (const headers of [{}, { authorization: "Bearer wrong" }, { authorization: "s3cret" }, { authorization: "Bearer " }]) {
    const { resp, db } = await run("/stats", { env, headers });
    assert.equal(resp.status, 401, JSON.stringify(headers));
    assert.equal(db.queries.length, 0);
  }
});

test("/stats returns the rows and per-day totals", async () => {
  const rows = [
    { day: "2026-11-12", version: "0.2.0", os: "linux", arch: "x86_64", channel: "aur", n: 5 },
    { day: "2026-11-12", version: "0.1.0", os: "macos", arch: "aarch64", channel: "dmg", n: 2 },
    { day: "2026-11-11", version: "0.1.0", os: "macos", arch: "aarch64", channel: "dmg", n: 3 },
  ];
  const { resp, body, db } = await run("/stats?days=7", {
    env: { STATS_TOKEN: "s3cret", DB: fakeDb(rows) },
    headers: { authorization: "Bearer s3cret" },
  });
  assert.equal(resp.status, 200);
  assert.equal(body.from, "2026-11-06");
  assert.equal(body.to, "2026-11-12");
  assert.deepEqual(body.per_day, [
    { day: "2026-11-12", n: 7 },
    { day: "2026-11-11", n: 3 },
  ]);
  assert.deepEqual(body.rows, rows);
  assert.deepEqual(db.queries[0].args, ["2026-11-06"]);
});

test("/stats clamps days", async () => {
  const env = { STATS_TOKEN: "t" };
  const headers = { authorization: "Bearer t" };
  assert.equal((await run("/stats?days=0", { env, headers })).body.from, "2026-11-12");
  assert.equal((await run("/stats?days=abc", { env, headers })).body.from, "2026-10-14");
  assert.equal((await run("/stats?days=100000", { env, headers })).body.from, day(new Date(NOW.getTime() - 399 * 86_400_000)));
});
