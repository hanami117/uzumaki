# Update endpoint

A Cloudflare Worker that answers Uzumaki's once-a-day update check and keeps an anonymous count of
those checks. It is the only thing Uzumaki ever sends anywhere on its own.

## What a check sends and what is kept

The app asks

    GET https://updates.getuzumaki.app/latest.json?v=0.2.0&os=linux&arch=x86_64&ch=appimage[&w=1][&m=1]

and gets back the site's `latest.json` (newest version, release date, notes, download links,
`minimum_supported`). The Worker adds one to a counter for that UTC day and that combination of
version, OS, architecture and install channel (`n`). The app adds `w=1` to its first check of an ISO
week and `m=1` to its first of a calendar month (UTC); those add one to two more counters in the same
row (`w`, `m`), so weekly and monthly users can be counted without telling installs apart. Those
counters (`schema.sql`) are all that is stored:

- no IP address, no install or device id, no cookie, no user agent, no time finer than the day;
- one row per combination per day, never one row per request;
- request logs and traces are off (`[observability] enabled = false`).

Cloudflare, which runs the Worker, sees each connection as any web host does; nothing about it is
written to the database. A request without a readable version and OS (a browser, say) is answered
and not counted, as is one with `repeat=1` (a second check on the same day).

Turning off "Check for updates" in Settings → General stops the request altogether.

## Reading the numbers

With wrangler logged in to the account:

```sh
# daily: installs that ran that day
npx wrangler d1 execute uzumaki-updates --remote --command \
  "SELECT day, SUM(n) AS daily FROM checks GROUP BY day ORDER BY day DESC LIMIT 30"
# weekly (Monday–Sunday, UTC): installs that ran that week
npx wrangler d1 execute uzumaki-updates --remote --command \
  "SELECT date(day, 'weekday 0', '-6 days') AS week, SUM(w) AS weekly FROM checks GROUP BY week ORDER BY week DESC LIMIT 12"
# monthly: installs that ran that month
npx wrangler d1 execute uzumaki-updates --remote --command \
  "SELECT substr(day, 1, 7) AS month, SUM(m) AS monthly FROM checks GROUP BY month ORDER BY month DESC LIMIT 12"
```

The current week and month are still filling up.

Or, after `npx wrangler secret put STATS_TOKEN`, `GET /stats?days=30` with
`Authorization: Bearer <token>`. Without that secret `/stats` does not exist.

## Development

```sh
npm test                                   # node --test, no dependencies
npx wrangler deploy --dry-run              # bundles and checks wrangler.toml, uploads nothing
```

Deploying needs a D1 database (`npx wrangler d1 create uzumaki-updates`, then its id in
`wrangler.toml`), the schema (`npx wrangler d1 execute uzumaki-updates --remote --file schema.sql`)
and `npx wrangler deploy`.
