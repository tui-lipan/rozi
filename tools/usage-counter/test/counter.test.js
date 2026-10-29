// Runs the real handler against a real local D1 database (wrangler's platform proxy, in memory).
// Only Cloudflare's rate limiter is replaced, since it exists only on Cloudflare.

import { after, before, beforeEach, describe, test } from "node:test";
import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { readFileSync, readdirSync } from "node:fs";
import { getPlatformProxy } from "wrangler";

import { handle } from "../src/index.js";
import { isoWeek, parseReport } from "../src/report.js";

const migrationsDir = new URL("../migrations/", import.meta.url);
// Every migration in order, one statement per string, the way `wrangler d1 migrations` applies them.
const statements = readdirSync(migrationsDir)
  .filter((name) => name.endsWith(".sql"))
  .sort()
  .flatMap((name) =>
    readFileSync(new URL(name, migrationsDir), "utf8")
      .replace(/--.*$/gm, "")
      .split(";")
      .map((statement) => statement.replace(/\s+/g, " ").trim())
      .filter(Boolean),
  );

// What `src/ops/usage_count.rs` sends; its tests check the same file.
const fixture = JSON.parse(readFileSync(new URL("./client-report.json", import.meta.url), "utf8"));

/** One install's report: the fixture with a token of its own, as each client makes. */
function report(fields = {}) {
  return { ...fixture, token: randomBytes(16).toString("hex"), ...fields };
}

const thursday = new Date("2026-10-29T12:00:00Z");

let proxy;
let env;

before(async () => {
  proxy = await getPlatformProxy({ persist: false });
});

after(async () => {
  await proxy?.dispose();
});

beforeEach(async () => {
  await proxy.env.DB.exec("DROP TABLE IF EXISTS weekly_installs");
  await proxy.env.DB.exec("DROP TABLE IF EXISTS report_tokens");
  for (const statement of statements) await proxy.env.DB.exec(statement);
  env = { DB: proxy.env.DB };
});

function post(body, headers = {}) {
  return new Request("https://usage.tui-lipan.dev/v1/count", {
    method: "POST",
    headers: { "content-type": "application/json", ...headers },
    body: typeof body === "string" ? body : JSON.stringify(body),
  });
}

async function send(body, now = thursday, headers = {}) {
  const response = await handle(post(body, headers), env, now);
  assert.equal(response.status, 200, await response.clone().text());
  return response.json();
}

async function rows() {
  const { results } = await env.DB.prepare("SELECT * FROM weekly_installs ORDER BY week").all();
  return results;
}

async function tokens() {
  const { results } = await env.DB.prepare("SELECT * FROM report_tokens ORDER BY week").all();
  return results;
}

describe("POST /v1/count", () => {
  test("adds each install to this week's row for its values", async () => {
    for (let i = 0; i < 3; i++) {
      assert.deepEqual(await send(report()), { week: "2026-W44", counted: true });
    }
    await send(report({ os: "macos", arch: "aarch64" }));

    const stored = await rows();
    assert.equal(stored.length, 2);
    const linux = stored.find((row) => row.os === "linux");
    assert.deepEqual(linux, {
      week: "2026-W44",
      app: "rozi",
      version: "0.0.37",
      channel: "release",
      os: "linux",
      arch: "x86_64",
      install: "managed",
      age: 1,
      installs: 3,
    });
  });

  // The client resends a report whose answer it never saw. Counting it again would count the
  // install twice.
  test("a resent report is answered with its week and not counted again", async () => {
    const once = report();
    assert.deepEqual(await send(once), { week: "2026-W44", counted: true });
    const later = new Date("2026-10-29T18:00:00Z");
    assert.deepEqual(await send(once, later), { week: "2026-W44", counted: false });

    // Resent after the week turned over, it still names the week that counted it, so the client
    // marks that week and makes a fresh report for the new one.
    const monday = new Date("2026-11-02T08:00:00Z");
    assert.deepEqual(await send(once, monday), { week: "2026-W44", counted: false });

    const [row] = await rows();
    assert.equal(row.installs, 1);
  });

  test("the reply names the counter's week, not the client's", async () => {
    // Sent on Sunday by the client's clock, it arrives on Monday.
    const arrival = new Date("2026-11-02T00:00:01Z");
    assert.deepEqual(await send(report(), arrival), { week: "2026-W45", counted: true });
  });

  test("stores nothing but the week and the reported values, and the token apart", async () => {
    const sent = report({ hostname: "laptop", user: "someone" });
    await send(sent, thursday, { "cf-connecting-ip": "192.0.2.7" });
    const [row] = await rows();
    assert.deepEqual(Object.keys(row).sort(), [
      "age",
      "app",
      "arch",
      "channel",
      "install",
      "installs",
      "os",
      "version",
      "week",
    ]);
    assert.deepEqual(await tokens(), [{ token: sent.token, week: "2026-W44" }]);
    const everything = JSON.stringify([await rows(), await tokens()]);
    assert.ok(!everything.includes("192.0.2.7"));
    assert.ok(!everything.includes("laptop"));
  });

  test("tokens are forgotten once their week is two weeks old", async () => {
    const old = report();
    await send(old, new Date("2026-10-15T12:00:00Z"));
    await send(report(), new Date("2026-10-22T12:00:00Z"));
    await send(report(), thursday);
    assert.deepEqual(
      (await tokens()).map((row) => row.week),
      ["2026-W43", "2026-W44"],
    );
  });

  test("refuses a report outside the contract and stores nothing", async () => {
    const refused = [
      "not json",
      "[]",
      report({ schema: 2 }),
      report({ app: "other" }),
      report({ version: "latest" }),
      report({ version: "1.2.3; DROP TABLE weekly_installs" }),
      report({ age: 0 }),
      report({ os: "Linux With Spaces" }),
      report({ install: "x".repeat(40) }),
      report({ token: undefined }),
      report({ token: "not-hex-and-too-short" }),
      report({ token: "0123456789ABCDEF0123456789ABCDEF" }),
    ];
    for (const body of refused) {
      const response = await handle(post(body), env, thursday);
      assert.equal(response.status, 400, JSON.stringify(body));
    }
    const large = await handle(post(report({ pad: "x".repeat(2000) })), env, thursday);
    assert.equal(large.status, 413);
    assert.deepEqual(await rows(), []);
    assert.deepEqual(await tokens(), []);
  });

  test("counts an unlisted but well-formed platform as other", async () => {
    await send(report({ os: "haiku", arch: "sparc64" }));
    const [row] = await rows();
    assert.equal(row.os, "other");
    assert.equal(row.arch, "other");
  });

  test("an address over its rate limit is told to retry, and nothing is stored", async () => {
    const keys = [];
    env.COUNT_LIMIT = {
      async limit({ key }) {
        keys.push(key);
        return { success: false };
      },
    };
    const response = await handle(
      post(report(), { "cf-connecting-ip": "192.0.2.7" }),
      env,
      thursday,
    );
    assert.equal(response.status, 429);
    assert.deepEqual(keys, ["192.0.2.7"]);
    assert.deepEqual(await rows(), []);
  });

  test("the method is checked", async () => {
    const response = await handle(
      new Request("https://usage.tui-lipan.dev/v1/count"),
      env,
      thursday,
    );
    assert.equal(response.status, 405);
  });
});

describe("GET /v1/week", () => {
  test("names the current week by the counter's clock and stores nothing", async () => {
    const sunday = new Date("2026-11-01T23:59:59Z");
    const monday = new Date("2026-11-02T00:00:00Z");
    for (const [now, week] of [
      [sunday, "2026-W44"],
      [monday, "2026-W45"],
    ]) {
      const response = await handle(new Request("https://usage.tui-lipan.dev/v1/week"), env, now);
      assert.equal(response.status, 200);
      assert.equal(response.headers.get("cache-control"), "no-store");
      assert.deepEqual(await response.json(), { week });
    }
    assert.deepEqual(await rows(), []);
    assert.deepEqual(await tokens(), []);
  });
});

describe("GET /v1/stats", () => {
  test("totals each week with its new installs and breakdowns", async () => {
    const lastWeek = new Date("2026-10-22T12:00:00Z");
    await send(report(), lastWeek);
    await send(report({ age: 2 }));
    await send(report({ age: 1, install: "cargo" }));
    await send(report({ age: 1, os: "windows" }));

    const response = await handle(
      new Request("https://usage.tui-lipan.dev/v1/stats?app=rozi&weeks=4"),
      env,
      thursday,
    );
    assert.equal(response.status, 200);
    assert.equal(response.headers.get("access-control-allow-origin"), "*");
    const body = await response.json();
    assert.equal(body.app, "rozi");
    assert.deepEqual(
      body.weeks.map(({ week, complete, installs, new_installs }) => ({
        week,
        complete,
        installs,
        new_installs,
      })),
      [
        { week: "2026-W43", complete: true, installs: 1, new_installs: 1 },
        { week: "2026-W44", complete: false, installs: 3, new_installs: 2 },
      ],
    );
    assert.deepEqual(body.weeks[1].by.install, { managed: 2, cargo: 1 });
    assert.deepEqual(body.weeks[1].by.os, { linux: 2, windows: 1 });
    assert.deepEqual(body.weeks[1].by.age, { 1: 2, 2: 1 });
  });

  test("leaves out weeks older than asked for", async () => {
    await send(report(), new Date("2026-01-08T12:00:00Z"));
    await send(report());
    const response = await handle(
      new Request("https://usage.tui-lipan.dev/v1/stats?weeks=2"),
      env,
      thursday,
    );
    const body = await response.json();
    assert.deepEqual(
      body.weeks.map((week) => week.week),
      ["2026-W44"],
    );
  });
});

describe("contract", () => {
  test("ISO weeks start on Monday and belong to the year of their Thursday", () => {
    assert.equal(isoWeek(new Date("2026-10-25T23:59:59Z")), "2026-W43");
    assert.equal(isoWeek(new Date("2026-10-26T00:00:00Z")), "2026-W44");
    assert.equal(isoWeek(new Date("2027-01-01T12:00:00Z")), "2026-W53");
    assert.equal(isoWeek(new Date("2025-12-29T12:00:00Z")), "2026-W01");
  });

  test("a newer client's extra fields are ignored rather than refused", () => {
    const { report: parsed, error } = parseReport(report({ terminal: "ghostty" }));
    assert.equal(error, undefined);
    assert.equal(parsed.terminal, undefined);
  });

  test("the fixture the client is checked against is a valid report", () => {
    assert.equal(parseReport(fixture).error, undefined);
  });

  test("nightly prerelease versions are accepted", () => {
    assert.ok(parseReport(report({ version: "0.0.38-nightly.20261029", channel: "nightly" })).report);
  });
});
