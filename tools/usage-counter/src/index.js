// The tui-lipan usage counter: counts weekly reports from apps and publishes the totals.
//
// POST /v1/count  adds one install to this week's count. 200 with `{ "week", "counted" }` when the
//                 report is taken: `counted` is false when its token was already counted, which is
//                 what makes a resent report safe. 400 for a report that does not match the
//                 contract, 429 when one address sends too fast.
// GET  /v1/stats  the weekly totals, public. `?app=rozi&weeks=26`.
//
// The request's address is used only as the rate-limit key and is never stored or logged. The
// week comes from this server's clock, so a client with a wrong clock cannot pick one.

import { MAX_BODY_BYTES, isoWeek, parseReport } from "./report.js";

const DOCS = "https://rozi.tui-lipan.dev/usage-count";
const DEFAULT_WEEKS = 26;
const MAX_WEEKS = 520;
const STATS_MAX_AGE = 3600;
const DIMENSIONS = ["version", "channel", "os", "arch", "install", "age"];
// How long a token is remembered. A client resends an unanswered report at its next check, hours
// later, so two weeks covers an install that goes offline mid-send and stays off for a while.
const TOKEN_WEEKS = 2;

export default {
  async fetch(request, env) {
    return handle(request, env, new Date());
  },
};

export async function handle(request, env, now) {
  const { pathname } = new URL(request.url);
  if (pathname === "/v1/count") {
    return request.method === "POST" ? count(request, env, now) : notAllowed("POST");
  }
  if (pathname === "/v1/stats") {
    return request.method === "GET" ? stats(request, env, now) : notAllowed("GET");
  }
  if (pathname === "/") {
    return text(
      200,
      `tui-lipan usage counter. It counts each install of a tui-lipan app at most once a week, ` +
        `with no ID, address, or personal data. What is sent and how to opt out: ${DOCS}\n` +
        `Totals: /v1/stats?app=rozi\n`,
    );
  }
  return text(404, "not found\n");
}

async function count(request, env, now) {
  const address = request.headers.get("cf-connecting-ip");
  if (env.COUNT_LIMIT && address) {
    const { success } = await env.COUNT_LIMIT.limit({ key: address });
    if (!success) return text(429, "too many reports\n");
  }
  const declared = Number(request.headers.get("content-length") ?? 0);
  if (declared > MAX_BODY_BYTES) return text(413, "report too large\n");
  const body = await request.text();
  if (new TextEncoder().encode(body).length > MAX_BODY_BYTES) {
    return text(413, "report too large\n");
  }
  let value;
  try {
    value = JSON.parse(body);
  } catch {
    return text(400, "body is not JSON\n");
  }
  const { token, report, error } = parseReport(value);
  if (error) return text(400, `${error}\n`);

  const week = isoWeek(now);
  const expired = isoWeek(new Date(now.getTime() - TOKEN_WEEKS * 7 * 86400000));
  // One transaction, so the count is raised and the token recorded together or not at all. The
  // increment runs only while the token is unknown, so a resent report changes nothing.
  const [, claimed, stored] = await env.DB.batch([
    env.DB.prepare(
      `INSERT INTO weekly_installs (week, app, version, channel, os, arch, install, age, installs)
       SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1
        WHERE NOT EXISTS (SELECT 1 FROM report_tokens WHERE token = ?9)
       ON CONFLICT DO UPDATE SET installs = installs + 1`,
    ).bind(
      week,
      report.app,
      report.version,
      report.channel,
      report.os,
      report.arch,
      report.install,
      report.age,
      token,
    ),
    env.DB.prepare(
      "INSERT INTO report_tokens (token, week) VALUES (?1, ?2) ON CONFLICT DO NOTHING",
    ).bind(token, week),
    env.DB.prepare("SELECT week FROM report_tokens WHERE token = ?1").bind(token),
    env.DB.prepare("DELETE FROM report_tokens WHERE week <= ?1").bind(expired),
  ]);
  return json(
    { week: stored.results[0]?.week ?? week, counted: claimed.meta.changes === 1 },
    { "cache-control": "no-store" },
  );
}

async function stats(request, env, now) {
  const url = new URL(request.url);
  const app = url.searchParams.get("app") ?? "rozi";
  const weeks = clampWeeks(url.searchParams.get("weeks"));
  const cache = typeof caches === "undefined" ? null : caches.default;
  const cached = await cache?.match(request);
  if (cached) return cached;

  const since = isoWeek(new Date(now.getTime() - (weeks - 1) * 7 * 86400000));
  const { results } = await env.DB.prepare(
    `SELECT week, version, channel, os, arch, install, age, installs
       FROM weekly_installs
      WHERE app = ?1 AND week >= ?2
      ORDER BY week`,
  )
    .bind(app, since)
    .all();

  const response = json({ app, docs: DOCS, weeks: summarize(results, isoWeek(now)) }, {
    "cache-control": `public, max-age=${STATS_MAX_AGE}`,
    "access-control-allow-origin": "*",
  });
  await cache?.put(request, response.clone());
  return response;
}

/** Fold stored rows into one entry per week: the total, the new installs, and each breakdown. */
export function summarize(rows, currentWeek) {
  const weeks = new Map();
  for (const row of rows) {
    let week = weeks.get(row.week);
    if (!week) {
      week = {
        week: row.week,
        complete: row.week !== currentWeek,
        installs: 0,
        new_installs: 0,
        by: Object.fromEntries(DIMENSIONS.map((dimension) => [dimension, {}])),
      };
      weeks.set(row.week, week);
    }
    week.installs += row.installs;
    if (row.age === 1) week.new_installs += row.installs;
    for (const dimension of DIMENSIONS) {
      const key = String(row[dimension]);
      week.by[dimension][key] = (week.by[dimension][key] ?? 0) + row.installs;
    }
  }
  return [...weeks.values()];
}

function clampWeeks(value) {
  const weeks = Number.parseInt(value ?? "", 10);
  if (!Number.isFinite(weeks) || weeks < 1) return DEFAULT_WEEKS;
  return Math.min(weeks, MAX_WEEKS);
}

function notAllowed(method) {
  return new Response("method not allowed\n", {
    status: 405,
    headers: { allow: method, "content-type": "text/plain; charset=utf-8" },
  });
}

function text(status, body) {
  return new Response(body, {
    status,
    headers: { "content-type": "text/plain; charset=utf-8", "cache-control": "no-store" },
  });
}

function json(value, headers) {
  return new Response(`${JSON.stringify(value)}\n`, {
    status: 200,
    headers: { "content-type": "application/json; charset=utf-8", ...headers },
  });
}
