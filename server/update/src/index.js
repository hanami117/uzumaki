// Uzumaki's update endpoint (a Cloudflare Worker).
//
//   GET /latest.json?v=0.2.0&os=linux&arch=x86_64&ch=appimage[&w=1][&m=1]
//
// answers what the newest release is (the site's own latest.json, passed through) and adds one to
// a counter for that day (UTC) and that (version, os, arch, channel). `w=1` (the app's first check
// of the ISO week, UTC) and `m=1` (its first of the calendar month) add one to two more counters in
// the same row, so weekly and monthly actives can be read without any id. Those counters are the
// only thing kept: no IP address, no install id, no cookie, no user agent, no time finer than the day.
// A request without a readable version and OS is answered but not counted, and so is one with
// `repeat=1` (the app sends it when it already checked earlier that day, e.g. "Check now").
//
//   GET /stats?days=30   (Authorization: Bearer <STATS_TOKEN>)
//
// returns the counters. Without the STATS_TOKEN secret set it does not exist (404).

/** Where the newest release is described: the site's static file, written by packaging/release.sh. */
export const DEFAULT_SOURCE = "https://getuzumaki.app/latest.json";
/** How long Cloudflare may reuse the fetched latest.json (seconds). */
const SOURCE_TTL = 300;

export const OS = new Set(["linux", "macos", "windows"]);
export const ARCH = new Set(["x86_64", "aarch64"]);
/** How the copy was installed (what the app can tell, or what its package told it at build time). */
export const CHANNEL = new Set(["dmg", "homebrew", "appimage", "aur", "system", "install-sh", "source"]);
/** `0.2.0`, `0.2.0-beta.1`; a leading `v` is dropped first. Anything else is not counted. */
const VERSION = /^\d{1,4}\.\d{1,4}\.\d{1,4}(-[0-9A-Za-z.-]{1,24})?$/;

const MAX_STATS_DAYS = 400;

/** The bucket a request counts in, or null when it is not counted. */
export function bucket(params) {
  if (params.get("repeat") === "1") return null;
  const version = (params.get("v") || "").replace(/^v/, "");
  const os = params.get("os") || "";
  if (!VERSION.test(version) || !OS.has(os)) return null;
  const pick = (value, allowed) => (allowed.has(value) ? value : "unknown");
  return {
    version,
    os,
    arch: pick(params.get("arch"), ARCH),
    channel: pick(params.get("ch"), CHANNEL),
    week: params.get("w") === "1" ? 1 : 0,
    month: params.get("m") === "1" ? 1 : 0,
  };
}

/** The UTC day, `2026-11-12`. */
export function day(now) {
  return now.toISOString().slice(0, 10);
}

/** A valid version string, or null. */
function version(v) {
  return typeof v === "string" && VERSION.test(v.replace(/^v/, "")) ? v : null;
}

/** latest.json as the app gets it: the site's file, with `minimum_supported` always present
 *  (null when the file has none or an unreadable one). Null when the file itself is unusable. */
export function normalize(latest) {
  if (!latest || typeof latest !== "object" || Array.isArray(latest) || !version(latest.version)) return null;
  return { ...latest, minimum_supported: version(latest.minimum_supported) };
}

const UPSERT =
  "INSERT INTO checks (day, version, os, arch, channel, n, w, m) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7) " +
  "ON CONFLICT (day, version, os, arch, channel) DO UPDATE SET n = n + 1, w = w + excluded.w, m = m + excluded.m";

export async function count(db, today, b) {
  await db.prepare(UPSERT).bind(today, b.version, b.os, b.arch, b.channel, b.week, b.month).run();
}

function json(body, status = 200, extra = {}) {
  return new Response(body === null ? null : JSON.stringify(body, null, 2) + "\n", {
    status,
    headers: {
      "content-type": "application/json; charset=utf-8",
      // Every check must reach the Worker to be counted; nothing in between may answer for it.
      "cache-control": "no-store",
      "x-content-type-options": "nosniff",
      ...extra,
    },
  });
}

async function latest(env, fetcher) {
  try {
    const resp = await fetcher(env.LATEST_SOURCE || DEFAULT_SOURCE, {
      cf: { cacheTtl: SOURCE_TTL, cacheEverything: true },
    });
    if (!resp.ok) return null;
    return normalize(await resp.json());
  } catch {
    return null;
  }
}

/** Equal strings, in a time that doesn't depend on where they differ. */
async function sameSecret(a, b) {
  const enc = new TextEncoder();
  const [x, y] = await Promise.all([a, b].map((s) => crypto.subtle.digest("SHA-256", enc.encode(s))));
  const u = new Uint8Array(x);
  const v = new Uint8Array(y);
  let diff = 0;
  for (let i = 0; i < u.length; i++) diff |= u[i] ^ v[i];
  return diff === 0;
}

async function stats(request, env, now) {
  if (!env.STATS_TOKEN) return json({ error: "not found" }, 404);
  const auth = request.headers.get("authorization") || "";
  const token = auth.startsWith("Bearer ") ? auth.slice(7) : "";
  if (!token || !(await sameSecret(token, env.STATS_TOKEN))) return json({ error: "unauthorized" }, 401);
  const asked = Number.parseInt(new URL(request.url).searchParams.get("days") || "30", 10);
  const days = Number.isFinite(asked) ? Math.min(Math.max(asked, 1), MAX_STATS_DAYS) : 30;
  const from = day(new Date(now.getTime() - (days - 1) * 86_400_000));
  const { results } = await env.DB.prepare(
    "SELECT day, version, os, arch, channel, n, w, m FROM checks WHERE day >= ?1 " +
      "ORDER BY day DESC, n DESC, version, os, arch, channel",
  )
    .bind(from)
    .all();
  const perDay = new Map();
  for (const r of results) {
    const t = perDay.get(r.day) || { day: r.day, n: 0, w: 0, m: 0 };
    t.n += r.n;
    t.w += r.w || 0;
    t.m += r.m || 0;
    perDay.set(r.day, t);
  }
  return json({
    from,
    to: day(now),
    per_day: [...perDay.values()],
    rows: results,
  });
}

/** The Worker, with the clock and the outgoing fetch passed in (tests pass their own). */
export async function handle(request, env, ctx, { now = new Date(), fetcher = fetch } = {}) {
  const url = new URL(request.url);
  if (request.method !== "GET" && request.method !== "HEAD") {
    return json({ error: "method not allowed" }, 405, { allow: "GET, HEAD" });
  }
  if (url.pathname === "/stats" && request.method === "GET") return stats(request, env, now);
  if (url.pathname !== "/latest.json") return json({ error: "not found" }, 404);

  if (request.method === "GET") {
    const b = bucket(url.searchParams);
    // Counted after the answer is sent; a failed write never fails the check.
    if (b && env.DB) ctx.waitUntil(count(env.DB, day(now), b).catch(() => {}));
  }
  const body = await latest(env, fetcher);
  if (!body) return json({ error: "latest.json is unavailable" }, 502);
  return json(request.method === "HEAD" ? null : body);
}

export default {
  fetch: (request, env, ctx) => handle(request, env, ctx),
};
