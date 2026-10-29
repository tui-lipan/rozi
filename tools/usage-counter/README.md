# Usage counter

The Cloudflare Worker behind `https://usage.tui-lipan.dev`. It counts how many machines run a
tui-lipan app each week and publishes the totals. rozi's side of the contract is
`src/ops/usage_count.rs`; what users are told is `docs/usage-count.md`.

## What it stores

One D1 table, `weekly_installs`: a counter per ISO week and combination of reported values
(app, version, channel, OS, architecture, install channel, install-age bucket). Nothing else is
kept: no request, address, user agent, or time finer than the week. The address is used only as
the rate-limit key, and Worker observability is off so no request log exists either.

Each value is checked against the vocabulary in `src/report.js`. A well-formed value the counter
does not list yet, such as a new OS, is stored as `other`, so it is counted rather than refused.
A report outside the contract gets a 400 and is not retried by the client.

## Reading the numbers

- `installs` for a complete week is the number of machines that ran the app that week with
  counting on. It undercounts: machines that opted out, run in CI, or were only used on the day
  they first showed the notice are not in it.
- `new_installs` is the part of that week's count in age bucket 1, machines counted for the first
  time within the last seven days.
- The current week is marked `"complete": false` and keeps rising until Sunday ends (UTC).
- Without an ID, nothing stops someone replaying reports. The rate limit caps that at 30 a minute
  per address; a sudden jump from one combination of values is the sign to look for.

```bash
curl -s 'https://usage.tui-lipan.dev/v1/stats?app=rozi&weeks=12' | jq '.weeks[] | {week, installs, new_installs}'
npx wrangler d1 execute tui-lipan-usage --remote \
  --command "SELECT week, SUM(installs) FROM weekly_installs GROUP BY week ORDER BY week"
```

## Develop

```bash
npm install
npm test        # the real handler against an in-memory local D1
npm run dev     # wrangler dev --local; apply migrations with --local first
```

`test/client-report.json` is the report rozi sends. Both test suites read it, so change it
together with `src/ops/usage_count.rs`.

## Deploy

The Worker, its D1 database `tui-lipan-usage`, and the `usage.tui-lipan.dev` custom domain live on
the Cloudflare account that owns `tui-lipan.dev`. From this directory, logged in with
`npx wrangler login`:

```bash
npm run deploy
```

It applies pending migrations to the remote database, then deploys the Worker. Check it with
`curl -s https://usage.tui-lipan.dev/v1/stats`. Schema changes are new numbered files in
`migrations/`; never edit an applied one.

The database is bound as `DB`. If a `wrangler d1` command offers to add a binding to
`wrangler.jsonc`, decline: a second entry under another name is never read.

## Changing the contract

- Adding a field: add it to the client and here, and to `client-report.json`. Old counters ignore
  unknown fields and old clients never send it, so either side can ship first.
- Adding a value to a vocabulary (a new install channel, OS, or app): add it to `src/report.js`
  and deploy before a release sends it. Until then it is stored as `other`.
- Changing what an existing field means: bump `schema` on both sides. A counter that does not know
  the schema refuses it, so deploy the counter first.
