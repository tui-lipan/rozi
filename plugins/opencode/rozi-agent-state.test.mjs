import assert from "node:assert/strict"
import test from "node:test"

import { AgentReporter, AgentState, reduceAgentEvent } from "./rozi-agent-state.js"

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

function transport(responses = []) {
  const sent = []
  return {
    sent,
    send: async (message) => {
      sent.push(message)
      return responses.shift() ?? { ok: true }
    },
  }
}

// The V1 plugin is an agent integration: it reports through `agent-report` for the detected
// OpenCode agent, never through `set-status`, which would make the pane a generic activity.
test("reports claim the pane as the opencode agent and release on unload", async () => {
  const wire = transport()
  const reporter = new AgentReporter({ env: paneEnv, transport: wire.send })
  assert.equal(await reporter.report(permission), true)
  assert.equal(await reporter.report(idle), true)
  await reporter.release()
  const [first, second, release] = wire.sent
  assert.equal(first.cmd, "agent-report")
  assert.equal(first.agent, "opencode")
  assert.equal(first.target, 7)
  assert.equal(first.source_pane, 7)
  assert.equal(first.source_session, "abc")
  assert.equal(first.state, "blocked")
  assert.equal(first.reason, "permission required")
  assert.equal(second.integration, first.integration)
  assert.ok(second.seq > first.seq)
  assert.equal("reason" in second, false)
  assert.deepEqual(
    { cmd: release.cmd, integration: release.integration, target: release.target },
    { cmd: "agent-release", integration: first.integration, target: 7 },
  )
  assert.ok(release.seq > second.seq)
  assert.equal(wire.sent.some((message) => message.cmd === "set-status"), false)
})

test("a refused report is retried, and a retired claim starts a new token", async () => {
  const wire = transport([
    { ok: false, error: "agent `opencode` is not the currently detected agent in pane 7" },
    { ok: false, error: "integration token belongs to a retired agent incarnation" },
  ])
  const reporter = new AgentReporter({ env: paneEnv, transport: wire.send })
  assert.equal(await reporter.report(working), false)
  assert.equal(await reporter.report(working), false)
  assert.equal(await reporter.report(working), true)
  assert.equal(wire.sent[0].integration, wire.sent[1].integration)
  assert.notEqual(wire.sent[2].integration, wire.sent[1].integration)
  assert.equal(wire.sent[2].seq, 1)

  const state = tracker()
  assert.deepEqual(reduce(state, "session.status", { sessionID: "a", status: { type: "busy" } }), working)
  state.forget()
  assert.deepEqual(
    reduce(state, "session.status", { sessionID: "a", status: { type: "busy" } }),
    working,
    "a forgotten aggregate is offered again",
  )
})

test("an unclaimed reporter releases nothing, and is inert outside a pane", async () => {
  const wire = transport()
  await new AgentReporter({ env: paneEnv, transport: wire.send }).release()
  assert.equal(await new AgentReporter({ env: {}, transport: wire.send }).report(working), false)
  assert.equal(wire.sent.length, 0)
})
