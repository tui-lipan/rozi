import net from "node:net"

// OpenCode is an agent, so this plugin declares itself one: it holds a `publish` stream with
// `kind: "agent"` and publishes a single row with the aggregate state. `rozi status` would be a
// generic activity, and `agent-report` only lands once rozi's screen detection has recognized
// OpenCode in the pane - a declared agent publisher needs neither. Closing the stream (plugin
// unload, OpenCode exit, or a crash) withdraws the row.
const ROW_ID = "opencode"
const ROW_TITLE = "OpenCode"
const ACK_TIMEOUT_MS = 1000

export class AgentPublisher {
  #env
  #connect
  #connection
  #ready
  #latest
  #closed = false

  constructor({ env = process.env, connect = (path) => net.createConnection(path) } = {}) {
    this.#env = env
    this.#connect = connect
  }

  get pane() {
    const pane = Number.parseInt(this.#env.ROZI_PANE ?? "", 10)
    return Number.isSafeInteger(pane) ? pane : undefined
  }

  // Publish one aggregate state. Resolves to whether the stream is open and carried it, so a
  // caller can offer the same state again on its next event after rozi was unreachable.
  async publish({ status, reason }) {
    this.#latest = { status, reason }
    if (this.#closed) return false
    this.#ready ??= this.#open()
    const open = await this.#ready
    if (open) this.#write()
    return open
  }

  close() {
    this.#closed = true
    this.#connection?.end()
    this.#connection = undefined
    this.#ready = undefined
  }

  #request() {
    const request = { cmd: "publish", kind: "agent", name: ROW_ID, source_pane: this.pane }
    if (this.#env.ROZI_SESSION_INSTANCE) request.source_session = this.#env.ROZI_SESSION_INSTANCE
    return request
  }

  #open() {
    const socket = this.#env.ROZI_SOCKET
    if (!socket || this.pane === undefined) return Promise.resolve(false)
    return new Promise((resolve) => {
      let acknowledged = false
      let buffer = ""
      const connection = this.#connect(socket)
      const settle = (open) => {
        clearTimeout(timer)
        resolve(open)
      }
      const drop = () => {
        if (this.#connection === connection) {
          this.#connection = undefined
          this.#ready = undefined
        }
        if (!acknowledged) settle(false)
        acknowledged = true
      }
      const timer = setTimeout(() => {
        connection.destroy()
        drop()
      }, ACK_TIMEOUT_MS)
      this.#connection = connection
      connection.once("connect", () => connection.write(`${JSON.stringify(this.#request())}\n`))
      // Keep reading for the stream's whole life: rozi drops a publisher that stops reading.
      // Activations need no answer, since rozi focuses the pane and there is only one row.
      connection.on("data", (chunk) => {
        if (acknowledged) return
        buffer += chunk
        const end = buffer.indexOf("\n")
        if (end < 0) return
        acknowledged = true
        let ok = false
        try {
          ok = JSON.parse(buffer.slice(0, end)).ok === true
        } catch {}
        if (!ok) {
          connection.destroy()
          if (this.#connection === connection) {
            this.#connection = undefined
            this.#ready = undefined
          }
        }
        settle(ok)
      })
      connection.on("error", drop)
      connection.on("close", drop)
    })
  }

  #write() {
    const latest = this.#latest
    if (!latest || !this.#connection) return
    const row = { id: ROW_ID, title: ROW_TITLE, status: latest.status, active: true }
    if (latest.reason) row.reason = latest.reason
    this.#connection.write(`${JSON.stringify({ rows: [row] })}\n`)
  }
}

export const RoziAgentState = async () => {
  const state = new AgentState()
  const publisher = new AgentPublisher()
  await publisher.publish({ status: "idle" })

  return {
    event: async ({ event }) => {
      const update = reduceAgentEvent(state, event)
      // An unpublished state is forgotten so the next event offers the aggregate again.
      if (update && !(await publisher.publish(update))) state.forget()
    },
    dispose: async () => {
      state.clear()
      publisher.close()
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
