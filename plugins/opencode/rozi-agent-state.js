import { randomUUID } from "node:crypto"
import net from "node:net"

// OpenCode is an agent, so this plugin speaks through rozi's agent report path rather than a plain
// pane status: `rozi status` is a generic activity and never makes a pane an agent. Each plugin
// instance claims the pane under its own integration token for the `opencode` agent rozi detects
// in it, and releases the claim on unload.
const AGENT = "opencode"

function send(message, { socket = process.env.ROZI_SOCKET } = {}) {
  if (!socket) return Promise.resolve({ ok: false })
  return new Promise((resolve) => {
    let reply = ""
    let settled = false
    const connection = net.createConnection(socket)
    const finish = (response) => {
      if (settled) return
      settled = true
      connection.destroy()
      resolve(response)
    }
    connection.setTimeout(1000, () => finish({ ok: false }))
    connection.once("error", () => finish({ ok: false }))
    connection.on("data", (chunk) => {
      reply += chunk
      const end = reply.indexOf("\n")
      if (end < 0) return
      try {
        finish(JSON.parse(reply.slice(0, end)))
      } catch {
        finish({ ok: false })
      }
    })
    connection.once("connect", () => connection.write(`${JSON.stringify(message)}\n`))
    connection.once("close", () => finish({ ok: false }))
  })
}

// One plugin instance's claim on its pane: the integration token and sequence `agent-report`
// fences on. Transport is injected so the protocol is testable without a socket.
export class AgentReporter {
  #send
  #env
  #token = randomUUID()
  #seq = 0
  #claimed = false

  constructor({ env = process.env, transport = send } = {}) {
    this.#env = env
    this.#send = transport
  }

  get pane() {
    const pane = Number.parseInt(this.#env.ROZI_PANE ?? "", 10)
    return Number.isSafeInteger(pane) ? pane : undefined
  }

  #envelope(command) {
    const message = { ...command, target: this.pane, source_pane: this.pane }
    if (this.#env.ROZI_SESSION_INSTANCE) message.source_session = this.#env.ROZI_SESSION_INSTANCE
    return message
  }

  // Report one aggregate state. Resolves to whether rozi accepted it, so a caller can retry the
  // same state on its next event - before rozi has detected OpenCode in the pane, say.
  async report({ status, reason }) {
    if (this.pane === undefined || !this.#env.ROZI_SOCKET) return false
    this.#seq += 1
    const message = this.#envelope({
      cmd: "agent-report",
      agent: AGENT,
      integration: this.#token,
      state: status,
      seq: this.#seq,
    })
    if (reason) message.reason = reason
    const response = await this.#send(message, { socket: this.#env.ROZI_SOCKET })
    if (response?.ok) {
      this.#claimed = true
      return true
    }
    // rozi retires a claim when the pane's program is replaced. A retired token can never claim
    // again, so a restarted OpenCode in the same pane starts a new one.
    if (typeof response?.error === "string" && response.error.includes("retired")) {
      this.#token = randomUUID()
      this.#seq = 0
      this.#claimed = false
    }
    return false
  }

  async release() {
    if (!this.#claimed) return
    this.#claimed = false
    this.#seq += 1
    await this.#send(
      this.#envelope({ cmd: "agent-release", integration: this.#token, seq: this.#seq }),
      { socket: this.#env.ROZI_SOCKET },
    )
  }
}

export const RoziAgentState = async () => {
  const state = new AgentState()
  const reporter = new AgentReporter()

  return {
    event: async ({ event }) => {
      const update = reduceAgentEvent(state, event)
      // An unaccepted report is forgotten so the next event offers the aggregate again.
      if (update && !(await reporter.report(update))) state.forget()
    },
    dispose: async () => {
      state.clear()
      await reporter.release()
    },
  }
}

const BLOCKED_REASON = { permission: "permission required", question: "answer required" }

// Every OpenCode session behind one pane, and the single status that pane publishes for them.
//
// A pane is one PTY but OpenCode is many sessions: a parent, its subagents, and anything else
// open. Each reports its own lifecycle events, so deriving the pane's status from whichever event
// arrived last made a subagent going idle publish `idle` for the whole pane while the parent was
// still working - and because a reported status outranks rozi's own screen detection, that read
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

  // Drop what the pane was last told, so the next settle reports the aggregate even unchanged.
  forget() {
    this.#published = undefined
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
