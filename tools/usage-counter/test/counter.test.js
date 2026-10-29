// Runs the real handler against a real local D1 database (wrangler's platform proxy, in memory).
// Only Cloudflare's rate limiter is replaced, since it exists only on Cloudflare.

import { after, before, beforeEach, describe, test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { getPlatformProxy } from "wrangler";

import { handle } from "../src/index.js";
import { isoWeek, parseReport } from "../src/report.js";

const migration = readFileSync(
  new URL("../migrations/0001_weekly_installs.sql", import.meta.url),
  "utf8",
);

// What `src/ops/usage_count.rs` sends; its tests check the same file.
const report = JSON.parse(readFileSync(new URL("./client-report.json", import.meta.url), "utf8"));

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
  // `exec` takes one statement per line.
  await proxy.env.DB.exec(migration.replace(/--.*$/gm, "").replace(/\s+/g, " ").trim());
  env = { DB: proxy.env.DB };
});

function post(body, headers = {}) {
  return new Request("https://usage.tui-lipan.dev/v1/count", {
    method: "POST",
    headers: { "content-type": "application/json", ...headers },
    body: typeof body === "string" ? body : JSON.stringify(body),
  });
}

async function rows() {
  const { results } = await env.DB.prepare("SELECT * FROM weekly_installs ORDER BY week").all();
  return results;
}

describe("POST /v1/count", () => {
  test("adds the machine to this week's row for its values", async () => {
    for (let i = 0; i < 3; i++) {
      const response = await handle(post(report), env, thursday);
      assert.equal(response.status, 204);
    }
    await handle(post({ ...report, os: "macos", arch: "aarch64" }), env, thursday);

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

  test("stores nothing but the week and the reported values", async () => {
    await handle(
      post({ ...report, hostname: "laptop", user: "someone" }, { "cf-connecting-ip": "192.0.2.7" }),
      env,
      thursday,
    );
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
    assert.ok(!JSON.stringify(row).includes("192.0.2.7"));
  });

  test("refuses a report outside the contract and stores nothing", async () => {
    const refused = [
      "not json",
      "[]",
      { ...report, schema: 2 },
      { ...report, app: "other" },
      { ...report, version: "latest" },
      { ...report, version: "1.2.3; DROP TABLE weekly_installs" },
      { ...report, age: 0 },
      { ...report, os: "Linux With Spaces" },
      { ...report, install: "x".repeat(40) },
    ];
    for (const body of refused) {
      const response = await handle(post(body), env, thursday);
      assert.equal(response.status, 400, JSON.stringify(body));
    }
    const large = await handle(post({ ...report, pad: "x".repeat(2000) }), env, thursday);
    assert.equal(large.status, 413);
    assert.deepEqual(await rows(), []);
  });

  test("counts an unlisted but well-formed platform as other", async () => {
    await handle(post({ ...report, os: "haiku", arch: "sparc64" }), env, thursday);
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
    const response = await handle(post(report, { "cf-connecting-ip": "192.0.2.7" }), env, thursday);
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

describe("GET /v1/stats", () => {
  test("totals each week with its new installs and breakdowns", async () => {
    const lastWeek = new Date("2026-10-22T12:00:00Z");
    await handle(post(report), env, lastWeek);
    await handle(post({ ...report, age: 2 }), env, thursday);
    await handle(post({ ...report, age: 1, install: "cargo" }), env, thursday);
    await handle(post({ ...report, age: 1, app: "rozi", os: "windows" }), env, thursday);

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
    await handle(post(report), env, new Date("2026-01-08T12:00:00Z"));
    await handle(post(report), env, thursday);
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
    const { report: parsed, error } = parseReport({ ...report, terminal: "ghostty" });
    assert.equal(error, undefined);
    assert.equal(parsed.terminal, undefined);
  });

  test("nightly prerelease versions are accepted", () => {
    assert.ok(parseReport({ ...report, version: "0.0.38-nightly.20261029", channel: "nightly" }).report);
  });
});
