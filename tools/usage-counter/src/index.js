// The tui-lipan usage counter: counts weekly reports from apps and publishes the totals.
//
// POST /v1/count  adds one machine to this week's count. 204 when counted, 400 for a report that
//                 does not match the contract, 429 when one address sends too fast.
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
      `tui-lipan usage counter. It counts each machine running a tui-lipan app at most once a week, ` +
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
  const { report, error } = parseReport(value);
  if (error) return text(400, `${error}\n`);

  await env.DB.prepare(
    `INSERT INTO weekly_installs (week, app, version, channel, os, arch, install, age, installs)
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1)
     ON CONFLICT DO UPDATE SET installs = installs + 1`,
  )
    .bind(
      isoWeek(now),
      report.app,
      report.version,
      report.channel,
      report.os,
      report.arch,
      report.install,
      report.age,
    )
    .run();
  return new Response(null, { status: 204, headers: { "cache-control": "no-store" } });
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
