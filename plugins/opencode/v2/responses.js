const MAX_BYTES = 1024 * 1024

export function finalResponse(messages) {
  const ordered = [...messages].sort((a, b) => (a.time?.created ?? 0) - (b.time?.created ?? 0))
  const last = ordered.findLast((message) => message.type === "assistant" || message.type === "user")
  if (last?.type !== "assistant" || !last.time?.completed || last.error) return
  const text = (last.content ?? []).filter((part) => part.type === "text").map((part) => part.text).join("")
  if (!text) return
  const bytes = Buffer.from(text)
  // Decode only complete code points. A truncation is explicit in the public response.
  let end = Math.min(bytes.length, MAX_BYTES)
  while (end < bytes.length && (bytes[end] & 0xc0) === 0x80) end--
  const truncated = bytes.length > end
  return { text: bytes.subarray(0, end).toString("utf8"), turn_id: last.id,
    source: "native-api", fidelity: "semantic", complete: !truncated, truncated }
}

// Holds the completed lifecycle transition until its message sync finishes, so a waiter cannot
// observe idle before the response. Tickets discard a fetch if that session starts another turn.
export class SessionResponses {
  #sessions = new Map()
  #stopped = false

  constructor(context, refresh) { this.context = context; this.refresh = refresh }

  stop() { this.#stopped = true; this.#sessions.clear() }

  event(event) {
    const id = event.data?.sessionID
    if (typeof id !== "string" || this.context.data.session.root(id) !== id) return
    if (event.type === "session.execution.started") this.#working(id)
    if (["session.execution.failed", "session.execution.interrupted"].includes(event.type)) {
      this.#sessions.set(id, { seq: this.#sessions.get(id)?.seq ?? 0, working: false })
    }
    if (event.type === "session.execution.succeeded") this.#read(id)
  }

  rows(rows) {
    const ids = new Set(rows.map((row) => row.id))
    for (const id of this.#sessions.keys()) if (!ids.has(id)) this.#sessions.delete(id)
    return rows.map((row) => {
      const previous = this.#sessions.get(row.id)
      if (row.status === "working") {
        if (!previous?.working && !previous?.loading && !previous?.response) this.#working(row.id)
      } else if (previous?.working && row.status !== "blocked") this.#read(row.id)
      const current = this.#sessions.get(row.id)
      return current?.loading ? { ...row, status: "working" } : row
    })
  }

  take(id) {
    const current = this.#sessions.get(id)
    if (!current?.response) return
    const result = { id, seq: current.seq, response: current.response }
    current.response = undefined
    return result
  }

  #working(id) {
    const old = this.#sessions.get(id)
    this.#sessions.set(id, { seq: old?.seq ?? 0, working: true })
  }

  #read(id) {
    const old = this.#sessions.get(id)
    if (old?.loading) return
    const messages = this.context.data.session.message
    if (!messages) return
    const current = { seq: (old?.seq ?? 0) + 1, loading: true, working: false }
    this.#sessions.set(id, current)
    // A failed sync must not leave the lifecycle stuck forever or reject the plugin event loop.
    let timer
    const timeout = new Promise((_, reject) => { timer = setTimeout(() => reject(new Error("message sync timeout")), 5000) })
    void Promise.race([Promise.resolve().then(() => messages.sync(id)), timeout])
      .then(() => {
        if (!this.#stopped && this.#sessions.get(id) === current) current.response = finalResponse(messages.list(id) ?? [])
      })
      .catch(() => {})
      .finally(() => {
        clearTimeout(timer)
        if (this.#stopped || this.#sessions.get(id) !== current) return
        current.loading = false
        this.refresh()
      })
  }
}
