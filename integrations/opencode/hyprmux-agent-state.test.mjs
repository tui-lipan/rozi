import assert from "node:assert/strict"
import test from "node:test"

import { AttentionTracker, reduceAgentEvent } from "./hyprmux-agent-state.js"

const event = (type, properties) => ({ type, properties })
const tracker = () => new AttentionTracker()
const reduce = (state, type, properties) => reduceAgentEvent(state, event(type, properties))
const working = { status: "working", reason: undefined }
const idle = { status: "idle", reason: undefined }

test("asked attention suppresses that session's busy and idle events", () => {
  const state = tracker()
  assert.deepEqual(
    reduce(state, "permission.v2.asked", { id: "permission-1", sessionID: "a" }),
    { status: "blocked", reason: "permission required" },
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
  assert.deepEqual(
    reduce(state, "session.error", { sessionID: "a" }),
    { status: "blocked", reason: "session error" },
  )
  assert.deepEqual(
    reduce(state, "session.status", { sessionID: "a", status: { type: "busy" } }),
    working,
  )
})

test("one reply leaves overlapping attention blocked", () => {
  const state = tracker()
  reduce(state, "permission.v2.asked", { id: "permission-1", sessionID: "a" })
  reduce(state, "question.asked", { id: "question-1", sessionID: "a" })
  assert.equal(
    reduce(state, "permission.v2.replied", { requestID: "permission-1", sessionID: "a" }),
    null,
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

test("attention is isolated to its session", () => {
  const state = tracker()
  reduce(state, "question.asked", { id: "question-1", sessionID: "a" })
  assert.equal(reduce(state, "session.status", { sessionID: "a", status: { type: "busy" } }), null)
  assert.deepEqual(
    reduce(state, "session.status", { sessionID: "b", status: { type: "busy" } }),
    working,
  )
  assert.deepEqual(reduce(state, "session.idle", { sessionID: "b" }), idle)
})

test("experimental v2 permission events use the same request identity lifecycle", () => {
  const state = tracker()
  assert.deepEqual(
    reduce(state, "permission.v2.asked", { id: "permission-1", sessionID: "a" }),
    { status: "blocked", reason: "permission required" },
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
    { status: "blocked", reason: "permission required" },
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
  assert.equal(reduce(state, "session.deleted", { sessionID: "a" }), null)
  assert.deepEqual(reduce(state, "session.idle", { sessionID: "a" }), idle)
  assert.equal(reduce(state, "session.idle", { sessionID: "b" }), null)
})
