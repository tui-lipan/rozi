import assert from "node:assert/strict"
import test from "node:test"

import { EventEmitter } from "node:events"

import {
  AgentPublisher,
  AgentState,
  agentStateHooks,
  reduceAgentEvent,
} from "./rozi-agent-state.js"

const event = (type, properties) => ({ type, properties })
const tracker = () => new AgentState()
const reduce = (state, type, properties) => reduceAgentEvent(state, event(type, properties))
const working = { status: "working", reason: undefined }
const idle = { status: "idle", reason: undefined }
const permission = { status: "blocked", reason: "permission required" }
const question = { status: "blocked", reason: "answer required" }

test("asked attention suppresses that session's busy and idle events", () => {
  const state = tracker()
  assert.deepEqual(
    reduce(state, "permission.v2.asked", { id: "permission-1", sessionID: "a" }),
    permission,
  )
  assert.equal(reduce(state, "session.status", { sessionID: "a", status: { type: "busy" } }), null)
  assert.equal(reduce(state, "session.idle", { sessionID: "a" }), null)
  assert.deepEqual(
    reduce(state, "permission.v2.replied", { requestID: "permission-1", sessionID: "a" }),
    working,
  )
  assert.deepEqual(reduce(state, "session.idle", { sessionID: "a" }), idle)
})

test("session errors are non-latching", () => {
  const state = tracker()
  assert.deepEqual(reduce(state, "session.error", { sessionID: "a" }), {
    status: "blocked",
    reason: "session error",
  })
  assert.deepEqual(
    reduce(state, "session.status", { sessionID: "a", status: { type: "busy" } }),
    working,
  )
})

test("one reply leaves overlapping attention blocked", () => {
  const state = tracker()
  reduce(state, "permission.v2.asked", { id: "permission-1", sessionID: "a" })
  reduce(state, "question.asked", { id: "question-1", sessionID: "a" })
  assert.deepEqual(
    reduce(state, "permission.v2.replied", { requestID: "permission-1", sessionID: "a" }),
    // Still blocked, but now by the question rather than the permission.
    question,
  )
  assert.deepEqual(
    reduce(state, "question.replied", { requestID: "question-1", sessionID: "a" }),
    working,
  )
})

test("out-of-order and mismatched resolutions leave requests intact", () => {
  const state = tracker()
  reduce(state, "permission.v2.asked", { id: "permission-1", sessionID: "a" })
  reduce(state, "permission.v2.asked", { id: "permission-2", sessionID: "a" })
  assert.equal(
    reduce(state, "question.replied", { requestID: "permission-1", sessionID: "a" }),
    null,
  )
  assert.equal(
    reduce(state, "permission.v2.replied", { requestID: "unknown", sessionID: "a" }),
    null,
  )
  assert.equal(
    reduce(state, "permission.v2.replied", { requestID: "permission-2", sessionID: "a" }),
    null,
  )
  assert.equal(reduce(state, "session.status", { sessionID: "a", status: { type: "idle" } }), null)
  assert.deepEqual(
    reduce(state, "permission.v2.replied", { requestID: "permission-1", sessionID: "a" }),
    working,
  )
})

test("a blocked session outranks another session's work", () => {
  const state = tracker()
  reduce(state, "question.asked", { id: "question-1", sessionID: "a" })
  assert.equal(
    reduce(state, "session.status", { sessionID: "b", status: { type: "busy" } }),
    null,
    "b working does not clear a's prompt",
  )
  assert.equal(reduce(state, "session.idle", { sessionID: "b" }), null)
  assert.deepEqual(
    reduce(state, "question.replied", { requestID: "question-1", sessionID: "a" }),
    working,
  )
})

// The reported bug. A subagent finishing while its parent still works published `idle` for the
// whole pane, and a reported status outranks screen detection, so the pane read as a finished run.
test("a subagent going idle does not end the parent's run", () => {
  const state = tracker()
  assert.deepEqual(
    reduce(state, "session.status", { sessionID: "parent", status: { type: "busy" } }),
    working,
  )
  assert.equal(
    reduce(state, "session.status", { sessionID: "child", status: { type: "busy" } }),
    null,
    "a second working session does not change the pane's status",
  )
  assert.equal(
    reduce(state, "session.idle", { sessionID: "child" }),
    null,
    "the subagent finishing must not publish idle while the parent works",
  )
  assert.deepEqual(
    reduce(state, "session.idle", { sessionID: "parent" }),
    idle,
    "the pane goes idle only once every session has",
  )
})

test("experimental v2 permission events use the same request identity lifecycle", () => {
  const state = tracker()
  assert.deepEqual(
    reduce(state, "permission.v2.asked", { id: "permission-1", sessionID: "a" }),
    permission,
  )
  assert.deepEqual(
    reduce(state, "permission.v2.replied", { requestID: "permission-1", sessionID: "a" }),
    working,
  )
})

test("current permission events use the same request identity lifecycle", () => {
  const state = tracker()
  assert.deepEqual(
    reduce(state, "permission.asked", { id: "permission-1", sessionID: "a" }),
    permission,
  )
  assert.equal(reduce(state, "session.idle", { sessionID: "a" }), null)
  assert.deepEqual(
    reduce(state, "permission.replied", { requestID: "permission-1", sessionID: "a" }),
    working,
  )
})

test("session deletion clears only that session's stale attention", () => {
  const state = tracker()
  reduce(state, "permission.v2.asked", { id: "permission-1", sessionID: "a" })
  reduce(state, "question.asked", { id: "question-1", sessionID: "b" })
  assert.deepEqual(
    reduce(state, "session.deleted", { sessionID: "a" }),
    question,
    "deleting the permission-blocked session leaves the question-blocked one",
  )
  assert.deepEqual(reduce(state, "session.deleted", { sessionID: "b" }), idle)
})

test("an unchanged aggregate is not republished", () => {
  const state = tracker()
  assert.deepEqual(
    reduce(state, "session.status", { sessionID: "a", status: { type: "busy" } }),
    working,
  )
  assert.equal(
    reduce(state, "session.status", { sessionID: "a", status: { type: "busy" } }),
    null,
    "a busy session's event stream must not reconnect to the control socket per token",
  )
})

const paneEnv = { ROZI_PANE: "7", ROZI_SOCKET: "/run/rozi.sock", ROZI_SESSION_INSTANCE: "abc" }

// A fake control socket: records every line written, and answers the publish request with `ack`
// (`null` for no answer at all).
function socket({ ack = { ok: true } } = {}) {
  const connections = []
  const connect = (path) => {
    const connection = new EventEmitter()
    connection.path = path
    connection.lines = []
    connection.ended = false
    connection.write = (data) => {
      for (const line of data.split("\n").filter(Boolean)) connection.lines.push(JSON.parse(line))
      if (connection.lines.length === 1 && ack !== null) {
        queueMicrotask(() => connection.emit("data", `${JSON.stringify(ack)}\n`))
      }
      return true
    }
    connection.end = () => { connection.ended = true; connection.emit("close") }
    connection.destroy = () => connection.emit("close")
    connections.push(connection)
    queueMicrotask(() => connection.emit("connect"))
    return connection
  }
  return { connections, connect }
}

// The V1 plugin declares itself an agent publisher, so it is an agent whether or not rozi's screen
// detection recognizes OpenCode, and never a generic `set-status` activity.
test("publishes one agent row over a declared agent stream", async () => {
  const wire = socket()
  const publisher = new AgentPublisher({ env: paneEnv, connect: wire.connect })
  assert.equal(await publisher.publish(idle), true)
  assert.equal(await publisher.publish(permission), true)
  assert.equal(wire.connections.length, 1, "one stream for the plugin's life")
  const [connection] = wire.connections
  assert.equal(connection.path, "/run/rozi.sock")
  const [request, first, second] = connection.lines
  assert.deepEqual(request, {
    cmd: "publish",
    kind: "agent",
    name: "opencode",
    source_pane: 7,
    source_session: "abc",
  })
  assert.deepEqual(first, { rows: [{ id: "opencode", title: "OpenCode", status: "idle", active: true }] })
  assert.deepEqual(second.rows[0].status, "blocked")
  assert.deepEqual(second.rows[0].reason, "permission required")
  publisher.close()
  assert.equal(connection.ended, true, "closing the stream withdraws the row")
  assert.equal(await publisher.publish(working), false)
})

test("a refused or dropped stream is reopened by the next publish", async () => {
  const refused = socket({ ack: { ok: false, error: "pane 7 not found" } })
  const publisher = new AgentPublisher({ env: paneEnv, connect: refused.connect })
  assert.equal(await publisher.publish(working), false)

  const wire = socket()
  const retried = new AgentPublisher({ env: paneEnv, connect: wire.connect })
  assert.equal(await retried.publish(working), true)
  wire.connections[0].emit("close")
  assert.equal(await retried.publish(idle), true)
  assert.equal(wire.connections.length, 2)
  assert.equal(wire.connections[1].lines.at(-1).rows[0].status, "idle")

  const state = tracker()
  assert.deepEqual(reduce(state, "session.status", { sessionID: "a", status: { type: "busy" } }), working)
  state.forget()
  assert.deepEqual(
    reduce(state, "session.status", { sessionID: "a", status: { type: "busy" } }),
    working,
    "a forgotten aggregate is offered again",
  )
})

test("a publisher outside a pane, or with no answer, publishes nothing", async () => {
  const wire = socket()
  assert.equal(await new AgentPublisher({ env: {}, connect: wire.connect }).publish(working), false)
  assert.equal(wire.connections.length, 0)
  const silent = socket({ ack: null })
  assert.equal(
    await new AgentPublisher({ env: paneEnv, connect: silent.connect }).publish(working),
    false,
  )
})

// Through the plugin's own event path: a stream that drops while a session works is reopened by
// the next event even when that event changes nothing, because rozi withdrew the row with it.
test("an unchanged busy event after a dropped stream republishes the row", async () => {
  const wire = socket()
  const hooks = await agentStateHooks({ env: paneEnv, connect: wire.connect })
  const busy = { type: "session.status", properties: { sessionID: "a", status: { type: "busy" } } }
  await hooks.event({ event: busy })
  assert.equal(wire.connections.length, 1)
  assert.deepEqual(
    wire.connections[0].lines.map((line) => line.rows?.[0]?.status),
    [undefined, "idle", "working"],
  )
  await hooks.event({ event: busy })
  assert.equal(wire.connections[0].lines.length, 3, "an unchanged state is not resent")

  wire.connections[0].emit("close")
  await hooks.event({ event: busy })
  assert.equal(wire.connections.length, 2, "the next event reopens the stream")
  assert.equal(wire.connections[1].lines.at(-1).rows[0].status, "working")

  await hooks.dispose()
  assert.equal(wire.connections[1].ended, true)
  await hooks.event({ event: busy })
  assert.equal(wire.connections.length, 2, "a disposed plugin opens nothing")
})
