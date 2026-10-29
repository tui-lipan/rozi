// What a usage report may contain, and the week it counts toward.
//
// The client side of this contract is `src/ops/usage_count.rs`. Every stored value comes from a
// short vocabulary or a bounded pattern, so the table cannot grow without limit and nothing a
// client sends can carry more than the field names say. Unknown fields are ignored, so a newer
// client can add one before this counter learns it.

export const SCHEMA_VERSION = 1;

export const APPS = new Set(["rozi"]);
export const CHANNELS = new Set(["release", "nightly"]);
export const INSTALLS = new Set([
  "managed",
  "cargo",
  "mise",
  "homebrew",
  "scoop",
  "winget",
  "system",
  "unknown",
]);
// `std::env::consts::OS` and `ARCH` values. Anything else that is shaped like one is kept as
// `other`, so a new platform is counted rather than refused.
export const OSES = new Set([
  "linux",
  "macos",
  "windows",
  "freebsd",
  "netbsd",
  "openbsd",
  "dragonfly",
  "illumos",
  "solaris",
  "android",
]);
export const ARCHES = new Set([
  "x86_64",
  "aarch64",
  "x86",
  "arm",
  "riscv64",
  "powerpc64",
  "s390x",
  "loongarch64",
]);
export const AGES = new Set([1, 2, 3, 4]);

const TOKEN = /^[a-z0-9_]{1,24}$/;
const VERSION = /^(0|[1-9]\d{0,4})\.(0|[1-9]\d{0,4})\.(0|[1-9]\d{0,4})(-[0-9A-Za-z.]{1,32})?$/;

export const MAX_BODY_BYTES = 1024;

/** A validated report, or `{ error }` naming the first field that is wrong. */
export function parseReport(value) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    return { error: "body is not an object" };
  }
  if (value.schema !== SCHEMA_VERSION) return { error: "unsupported schema" };
  if (!APPS.has(value.app)) return { error: "unknown app" };
  if (typeof value.version !== "string" || !VERSION.test(value.version)) {
    return { error: "invalid version" };
  }
  if (!AGES.has(value.age)) return { error: "invalid age" };
  const channel = known(CHANNELS, value.channel);
  const os = known(OSES, value.os);
  const arch = known(ARCHES, value.arch);
  const install = known(INSTALLS, value.install);
  if (!channel || !os || !arch || !install) return { error: "invalid field" };
  return {
    report: { app: value.app, version: value.version, channel, os, arch, install, age: value.age },
  };
}

/** The value itself when listed, `other` when it is shaped like one, else nothing. */
function known(allowed, value) {
  if (typeof value !== "string" || !TOKEN.test(value)) return null;
  return allowed.has(value) ? value : "other";
}

/** ISO 8601 week of `date` in UTC, such as `2026-W44`. */
export function isoWeek(date) {
  const day = new Date(Date.UTC(date.getUTCFullYear(), date.getUTCMonth(), date.getUTCDate()));
  const weekday = day.getUTCDay() || 7;
  // The Thursday of this week decides which year the week belongs to.
  day.setUTCDate(day.getUTCDate() + 4 - weekday);
  const yearStart = Date.UTC(day.getUTCFullYear(), 0, 1);
  const week = Math.ceil(((day - yearStart) / 86400000 + 1) / 7);
  return `${day.getUTCFullYear()}-W${String(week).padStart(2, "0")}`;
}
