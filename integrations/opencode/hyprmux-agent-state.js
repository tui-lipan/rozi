import net from "node:net"

const socket = process.env.HYPRMUX_SOCKET
const pane = Number.parseInt(process.env.HYPRMUX_PANE ?? "", 10)

function publish(status, reason) {
  if (!socket || !Number.isSafeInteger(pane)) return Promise.resolve()

  return new Promise((resolve) => {
    const connection = net.createConnection(socket)
    const finish = () => {
      connection.destroy()
      resolve()
    }
    connection.setTimeout(1000, finish)
    connection.once("error", resolve)
    connection.once("connect", () => {
      connection.end(`${JSON.stringify({
        cmd: "set-status",
        target: pane,
        status,
        reason,
        source_pane: pane,
      })}\n`)
    })
    connection.once("close", resolve)
  })
}

export const HyprmuxAgentState = async () => {
  const state = new AgentState()
  await publish("idle")

  return {
    event: async ({ event }) => {
      const update = reduceAgentEvent(state, event)
      if (update) await publish(update.status, update.reason)
    },
    dispose: async () => {
      state.clear()
      await publish("idle")
    },
  }
}

const BLOCKED_REASON = { permission: "permission required", question: "answer required" }

// Every OpenCode session behind one pane, and the single status that pane publishes for them.
//
// A pane is one PTY but OpenCode is many sessions: a parent, its subagents, and anything else
// open. Each reports its own lifecycle events, so deriving the pane's status from whichever event
// arrived last made a subagent going idle publish `idle` for the whole pane while the parent was
// still working - and because a reported status outranks hyprmux's own screen detection, that read
// as a finished run rather than as no information. The pane's status is an aggregate instead.
export class AgentState {
  #requests = new Map()
  #busy = new Map()
  #errored = new Set()
  #published

  add(sessionID, kind, id) {
    if (!requestIdentity(sessionID, id)) return false
    let byKind = this.#requests.get(sessionID)
    if (!byKind) this.#requests.set(sessionID, (byKind = new Map()))
    let ids = byKind.get(kind)
    if (!ids) byKind.set(kind, (ids = new Set()))
    const existed = ids.has(id)
    ids.add(id)
    // A session is only ever asked for permission or an answer part-way through a run, so an
    // unresolved prompt is itself evidence the session is working. Recording that here is what
    // lets the answer resume `working` rather than dropping to `idle` for the moment before the
    // next status event arrives - a dip that would read as a finished run.
    this.#busy.set(sessionID, true)
    return !existed
  }

  resolve(sessionID, kind, requestID) {
    if (!requestIdentity(sessionID, requestID)) return false
    const byKind = this.#requests.get(sessionID)
    const ids = byKind?.get(kind)
    if (!ids?.delete(requestID)) return false
    if (ids.size === 0) byKind.delete(kind)
    if (byKind.size === 0) this.#requests.delete(sessionID)
    return true
  }

  has(sessionID) {
    return this.#requests.has(sessionID)
  }

  // A session's own state, independent of every other session behind this pane.
  #sessionStatus(sessionID) {
    const byKind = this.#requests.get(sessionID)
    if (byKind) {
      // Permission and answer prompts both block; name whichever arrived, preferring permission
      // since it is the one that stops tool execution.
      const kind = byKind.has("permission") ? "permission" : "question"
      return { status: "blocked", reason: BLOCKED_REASON[kind] }
    }
    if (this.#errored.has(sessionID)) return { status: "blocked", reason: "session error" }
    if (this.#busy.get(sessionID)) return { status: "working", reason: undefined }
    return { status: "idle", reason: undefined }
  }

  // Severity order, not recency: the pane's row answers "is anything here demanding attention",
  // so one blocked session outranks any number of working ones and any working session outranks
  // the sessions that have finished.
  aggregate() {
    const sessions = new Set([
      ...this.#requests.keys(),
      ...this.#busy.keys(),
      ...this.#errored,
    ])
    let working = false
    for (const sessionID of sessions) {
      const state = this.#sessionStatus(sessionID)
      if (state.status === "blocked") return state
      if (state.status === "working") working = true
    }
    return working ? { status: "working", reason: undefined } : { status: "idle", reason: undefined }
  }

  // The aggregate, but only when it differs from what the pane already shows. Returning `null`
  // for an unchanged aggregate is what keeps a busy session's event stream from reconnecting to
  // the control socket on every token.
  settle() {
    const next = this.aggregate()
    if (this.#published && this.#published.status === next.status && this.#published.reason === next.reason) {
      return null
    }
    this.#published = next
    return next
  }

  setBusy(sessionID, busy) {
    if (typeof sessionID !== "string" || sessionID.length === 0) return
    // A session waiting on the user keeps whatever it was doing before the prompt. OpenCode still
    // reports status while a prompt is open, and letting an `idle` through here would resolve the
    // answer into a finished run instead of a resumed one.
    if (this.has(sessionID)) return
    this.#errored.delete(sessionID)
    if (busy) this.#busy.set(sessionID, true)
    else this.#busy.delete(sessionID)
  }

  setErrored(sessionID) {
    if (typeof sessionID !== "string" || sessionID.length === 0) return
    this.#errored.add(sessionID)
  }

  clearSession(sessionID) {
    if (typeof sessionID !== "string" || sessionID.length === 0) return
    this.#requests.delete(sessionID)
    this.#busy.delete(sessionID)
    this.#errored.delete(sessionID)
  }

  clear() {
    this.#requests.clear()
    this.#busy.clear()
    this.#errored.clear()
    this.#published = undefined
  }
}

// Pure event reducer for OpenCode's permission/question lifecycle.
//
// `null` means the pane's published status does not change - either the event moved a session the
// aggregate does not currently speak for, or it moved nothing at all.
export function reduceAgentEvent(state, event) {
  const properties = event?.properties ?? {}
  const sessionID = properties.sessionID
  switch (event?.type) {
    case "session.status":
      state.setBusy(sessionID, properties.status?.type !== "idle")
      break
    case "session.idle":
      state.setBusy(sessionID, false)
      break
    case "session.error":
      state.setErrored(sessionID)
      break
    case "permission.asked":
    case "permission.v2.asked":
      state.add(sessionID, "permission", properties.id)
      break
    case "question.asked":
      state.add(sessionID, "question", properties.id)
      break
    case "permission.replied":
    case "permission.v2.replied":
      state.resolve(sessionID, "permission", properties.requestID)
      break
    case "question.replied":
    case "question.rejected":
      state.resolve(sessionID, "question", properties.requestID)
      break
    case "session.deleted":
      state.clearSession(sessionID)
      break
    default:
      return null
  }
  return state.settle()
}

function requestIdentity(sessionID, id) {
  return (
    typeof sessionID === "string" &&
    sessionID.length > 0 &&
    typeof id === "string" &&
    id.length > 0
  )
}
