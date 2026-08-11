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
  const attention = new AttentionTracker()
  await publish("idle")

  return {
    event: async ({ event }) => {
      const update = reduceAgentEvent(attention, event)
      if (update) await publish(update.status, update.reason)
    },
    dispose: async () => {
      attention.clear()
      await publish("idle")
    },
  }
}

// Tracks actionable requests independently for every OpenCode session.
export class AttentionTracker {
  #requests = new Map()

  add(sessionID, kind, id) {
    if (!requestIdentity(sessionID, id)) return false
    let byKind = this.#requests.get(sessionID)
    if (!byKind) this.#requests.set(sessionID, (byKind = new Map()))
    let ids = byKind.get(kind)
    if (!ids) byKind.set(kind, (ids = new Set()))
    const existed = ids.has(id)
    ids.add(id)
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

  clearSession(sessionID) {
    if (typeof sessionID === "string" && sessionID.length > 0) this.#requests.delete(sessionID)
  }

  clear() {
    this.#requests.clear()
  }
}

// Pure event reducer for OpenCode's permission/question lifecycle.
// `null` means the existing pane status remains authoritative. In particular, unresolved
// attention suppresses that session's ordinary working/idle events without affecting others.
export function reduceAgentEvent(attention, event) {
  const properties = event?.properties ?? {}
  const sessionID = properties.sessionID
  switch (event?.type) {
    case "session.status":
      if (attention.has(sessionID)) return null
      return statusUpdate(properties.status?.type === "idle" ? "idle" : "working")
    case "session.idle":
      return attention.has(sessionID) ? null : statusUpdate("idle")
    case "session.error":
      return statusUpdate("blocked", "session error")
    case "permission.asked":
    case "permission.v2.asked":
      attention.add(sessionID, "permission", properties.id)
      return statusUpdate("blocked", "permission required")
    case "question.asked":
      attention.add(sessionID, "question", properties.id)
      return statusUpdate("blocked", "answer required")
    case "permission.replied":
    case "permission.v2.replied":
      return resolvedUpdate(attention, sessionID, "permission", properties.requestID)
    case "question.replied":
    case "question.rejected":
      return resolvedUpdate(attention, sessionID, "question", properties.requestID)
    case "session.deleted":
      attention.clearSession(sessionID)
      return null
    default:
      return null
  }
}

function requestIdentity(sessionID, id) {
  return (
    typeof sessionID === "string" &&
    sessionID.length > 0 &&
    typeof id === "string" &&
    id.length > 0
  )
}

function resolvedUpdate(attention, sessionID, kind, requestID) {
  return attention.resolve(sessionID, kind, requestID) && !attention.has(sessionID)
    ? statusUpdate("working")
    : null
}

function statusUpdate(status, reason) {
  return { status, reason }
}
