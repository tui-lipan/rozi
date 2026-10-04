import assert from "node:assert/strict"
import { EventEmitter } from "node:events"
import { PassThrough, Writable } from "node:stream"
import test from "node:test"
import { activate, SessionRows } from "./v2/rows.js"
import { startPublisher } from "./v2/publisher.js"
import serverPlugin from "./index.js"
import tuiPlugin from "./tui.js"

test("local directory entrypoints expose V2 definitions and are inert outside rozi", () => {
  assert.equal(serverPlugin.id, "rozi")
  assert.equal(serverPlugin.setup(), undefined)
  assert.equal(tuiPlugin.id, "rozi")
  // No context APIs are accessed before the pane environment has been checked.
  startPublisher(undefined, { env: {} })()
})

function fixture() {
  const sessions = new Map([
    ["a", { id: "a", title: "Main", location: { directory: "/repo/main" } }],
    ["b", { id: "b", title: "Feature", location: { directory: "/repo/feature" } }],
    ["child", { id: "child", parentID: "a", location: { directory: "/repo/main" } }],
    ["unrelated", { id: "unrelated", location: { directory: "/elsewhere" } }],
  ])
  const tabs = [{ sessionID: "a", active: true }, { sessionID: "b", active: false }]
  const statuses = new Map()
  const permissions = new Map()
  const forms = new Map()
  const syncs = []
  const focuses = []
  let listener = () => {}
  const context = {
    data: {
      listen(callback) { listener = callback; return () => { listener = () => {} } },
      session: {
        get: (id) => sessions.get(id),
        root: (id) => sessions.get(id)?.parentID ?? id,
        family: (id) => [...sessions.values()].filter((session) => session.parentID === id).map((s) => s.id),
        status: (id) => statuses.get(id) ?? "idle",
        permission: {
          list: (id) => permissions.get(id),
          async sync(id) { syncs.push(["permission", id]) },
        },
        form: {
          list: (id) => forms.get(id),
          async sync(id, location) { syncs.push(["form", id, location]) },
        },
      },
    },
    ui: {
      tabs: {
        enabled: () => true,
        list: () => tabs,
        focus(id) { focuses.push(id); for (const tab of tabs) tab.active = tab.sessionID === id },
      },
      router: { current: () => ({ type: "session", sessionID: "a" }), navigate: (route) => focuses.push(route) },
    },
  }
  return { context, sessions, tabs, statuses, permissions, forms, focuses, syncs,
    event: (type, data) => listener({ details: { type, data } }) }
}

test("only this client's tabs publish, each with its own checkout and conversation", () => {
  const f = fixture()
  const rows = new SessionRows().snapshot(f.context)
  assert.deepEqual(rows.map((r) => [r.id, r.cwd, r.native_session, r.active]), [
    ["a", "/repo/main", "a", true], ["b", "/repo/feature", "b", false],
  ])
})

test("clients sharing one session cache keep independent tab ownership", () => {
  const first = fixture()
  const second = fixture()
  second.sessions.clear()
  for (const [id, session] of first.sessions) second.sessions.set(id, session)
  second.tabs.splice(0, 2, { sessionID: "unrelated", active: true })
  assert.deepEqual(new SessionRows().snapshot(first.context).map((r) => r.id), ["a", "b"])
  assert.deepEqual(new SessionRows().snapshot(second.context).map((r) => r.id), ["unrelated"])
})

test("a child keeps its parent working and a background approval blocks only its family", () => {
  const f = fixture()
  f.statuses.set("child", "running")
  f.permissions.set("b", [{ id: "permission" }])
  const state = new SessionRows()
  assert.deepEqual(state.snapshot(f.context).map((r) => r.status), ["working", "blocked"])
  f.permissions.delete("b")
  f.forms.set("child", [{ id: "question" }])
  assert.equal(state.snapshot(f.context)[0].reason, "Input required")
  f.forms.delete("child")
  assert.equal(state.snapshot(f.context)[0].status, "working")
})

test("global input forms block only sessions at the form's location", () => {
  const f = fixture()
  f.context.data.session.form.list = (id, location) =>
    id === "global" && location?.directory === "/repo/main" ? [{ id: "mcp-input" }] : []
  assert.deepEqual(new SessionRows().snapshot(f.context).map((r) => r.status), ["blocked", "idle"])
})

test("background completion remains done until that tab is viewed", () => {
  const f = fixture()
  const state = new SessionRows()
  f.statuses.set("b", "running")
  state.snapshot(f.context)
  f.statuses.delete("b")
  assert.equal(state.snapshot(f.context)[1].status, "done")
  assert.equal(state.snapshot(f.context)[1].status, "done")
  activate(f.context, "b", state.snapshot(f.context))
  assert.equal(state.snapshot(f.context)[1].status, "idle")
})

test("a completed turn can be observed even between polling snapshots", () => {
  const f = fixture()
  const state = new SessionRows()
  state.event(f.context, { type: "session.execution.succeeded", data: { sessionID: "b" } })
  assert.equal(state.snapshot(f.context)[1].status, "done")
})

test("closed and deleted tabs withdraw, and unknown activations never open a session", () => {
  const f = fixture()
  f.sessions.delete("b")
  const state = new SessionRows()
  const rows = state.snapshot(f.context)
  assert.deepEqual(rows.map((r) => r.id), ["a"])
  activate(f.context, "unrelated", rows)
  activate(f.context, "b", rows)
  assert.deepEqual(f.focuses, [])
  f.tabs.length = 0
  assert.deepEqual(state.snapshot(f.context), [])
})

test("without tabs, a child route publishes its root and activation uses native navigation", () => {
  const f = fixture()
  f.context.ui.tabs.enabled = () => false
  f.context.ui.router.current = () => ({ type: "session", sessionID: "child" })
  const rows = new SessionRows().snapshot(f.context)
  assert.equal(rows.length, 1)
  assert.equal(rows[0].id, "a")
  activate(f.context, "a", rows)
  assert.deepEqual(f.focuses, [{ type: "session", sessionID: "a" }])
  f.context.ui.router.current = () => ({ type: "home" })
  assert.deepEqual(new SessionRows().snapshot(f.context), [])
})

test("moving a session updates its cwd and placeholder titles stay empty", () => {
  const f = fixture()
  const session = f.sessions.get("a")
  session.location.directory = "/repo/another-worktree"
  session.title = "New session - 2026-10-04"
  const row = new SessionRows().snapshot(f.context)[0]
  assert.equal(row.cwd, "/repo/another-worktree")
  assert.equal(row.title, "")
})

const env = { ROZI: "1", ROZI_PANE: "7", ROZI_BIN: "/rozi with spaces", ROZI_SOCKET: "endpoint",
  ROZI_SESSION_INSTANCE: "server-instance" }

function bridge({ slow = false } = {}) {
  const child = new EventEmitter()
  const writes = []
  const callbacks = []
  child.stdout = new PassThrough()
  child.stdin = new Writable({
    highWaterMark: slow ? 1 : 16384,
    write(data, _encoding, callback) {
      writes.push(JSON.parse(data.toString()))
      if (slow) callbacks.push(callback)
      else callback()
    },
  })
  child.kill = () => { child.killed = true }
  const launches = []
  return { child, writes, callbacks, launches,
    launch: (...args) => { launches.push(args); return child } }
}

const tick = () => new Promise((resolve) => setImmediate(resolve))

test("publisher launches the matching CLI, hydrates families, coalesces and activates", async (t) => {
  const f = fixture()
  const b = bridge()
  const stop = startPublisher(f.context, { env, launch: b.launch, interval: 10000 })
  t.after(stop)
  await tick()
  assert.equal(b.launches[0][0], env.ROZI_BIN)
  assert.deepEqual(b.launches[0][1], ["publish"])
  assert.equal(b.launches[0][2].env, env)
  assert.equal(b.writes.length, 1)
  assert.ok(f.syncs.some(([kind, id]) => kind === "permission" && id === "child"))
  f.event("session.message.text.delta", { sessionID: "a" })
  assert.equal(b.writes.length, 1)
  b.child.stdout.write('bad json\n{"activate":"unrelated"}\n{"activate":"b"}\n')
  assert.deepEqual(f.focuses, ["b"])
  assert.equal(b.writes.at(-1).rows[1].active, true)
  stop()
  stop()
  assert.equal(b.child.killed, true)
  assert.equal(b.child.stdin.writableEnded, true)
  const count = b.writes.length
  f.event("session.execution.succeeded", { sessionID: "a" })
  assert.equal(b.writes.length, count)
})

test("slow bridge writes keep only the latest pending snapshot", async (t) => {
  const f = fixture()
  const b = bridge({ slow: true })
  const stop = startPublisher(f.context, { env, launch: b.launch, interval: 10000 })
  t.after(stop)
  f.statuses.set("b", "running")
  f.event("session.status", { sessionID: "b" })
  f.permissions.set("b", [{ id: "permission" }])
  f.event("permission.asked", { sessionID: "b" })
  assert.equal(b.writes.length, 1)
  b.callbacks.shift()()
  await tick()
  assert.equal(b.writes.length, 2)
  assert.equal(b.writes[1].rows[1].status, "blocked")
  b.callbacks.shift()()
})

test("bridge failure stops publication silently", () => {
  const f = fixture()
  const b = bridge()
  startPublisher(f.context, { env, launch: b.launch, interval: 10000 })
  b.child.emit("error", new Error("missing executable"))
  assert.equal(b.child.killed, true)
})

test("publisher is inert outside a fully identified rozi pane", () => {
  for (const key of Object.keys(env)) {
    const b = bridge()
    const incomplete = { ...env }
    delete incomplete[key]
    startPublisher(fixture().context, { env: incomplete, launch: b.launch })()
    assert.equal(b.launches.length, 0)
  }
})
