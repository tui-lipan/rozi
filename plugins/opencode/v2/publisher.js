import { spawn } from "node:child_process"
import { createInterface } from "node:readline"
import { activate, SessionRows } from "./rows.js"
import { SessionResponses } from "./responses.js"

function insideRozi(env) {
  return env.ROZI === "1" && env.ROZI_BIN && env.ROZI_PANE && env.ROZI_SOCKET && env.ROZI_SESSION_INSTANCE
}

function readActivation(line) {
  try {
    const message = JSON.parse(line)
    return typeof message.activate === "string" ? message.activate : undefined
  } catch {
    return undefined
  }
}

export function startPublisher(context, { env = process.env, launch = spawn, interval = 250 } = {}) {
  if (!insideRozi(env)) return () => {}
  // Each row is an OpenCode session, so this publisher declares itself an agent: its rows are
  // listed by `rozi agents` and stand in for screen detection. The name keeps it apart from any
  // other publisher in the same pane.
  const child = launch(env.ROZI_BIN, ["publish", "--kind", "agent", "--name", "opencode"], {
    env,
    stdio: ["pipe", "pipe", "ignore"],
  })
  const input = createInterface({ input: child.stdout })
  const state = new SessionRows()
  const responses = new SessionResponses(context, refresh)
  const pendingResponses = new Map()
  const hydrated = new Set()
  let last
  let pending
  let writable = true
  let stopped = false
  let timer
  let unsubscribe = () => {}

  function stop() {
    if (stopped) return
    stopped = true
    clearInterval(timer)
    unsubscribe()
    responses.stop()
    input.close()
    // EOF withdraws this publisher's rows. Kill also covers a stuck bridge during unload.
    child.stdin.end()
    child.kill()
  }

  function drain() {
    while (!stopped && writable && pending !== undefined) {
      const content = pendingResponses.values().next().value
      if (content) pendingResponses.delete(content.id)
      // Keep other completions working until their own response can travel in the same line.
      const rows = pending.map((row) => pendingResponses.has(row.id) ? { ...row, status: "working" } : row)
      if (!pendingResponses.size) pending = undefined
      writable = child.stdin.write(JSON.stringify({ rows, ...(content ? { responses: [content] } : {}) }) + "\n")
    }
  }

  function hydrate(rows) {
    for (const row of rows) {
      const family = new Set([row.id, ...context.data.session.family(row.id)])
      for (const id of family) hydrateSession(id)
      hydrateGlobalForms(context.data.session.get(row.id).location)
    }
  }

  function hydrateSession(id) {
    if (hydrated.has(id)) return
    hydrated.add(id)
    const data = context.data.session
    const location = data.get(id)?.location
    void Promise.all([data.permission.sync(id), data.form.sync(id, location)])
      .then(refresh)
      .catch(() => hydrated.delete(id))
  }

  function hydrateGlobalForms(location) {
    const key = `global:${location.directory}`
    if (hydrated.has(key)) return
    hydrated.add(key)
    void context.data.session.form.sync("global", location)
      .then(refresh)
      .catch(() => hydrated.delete(key))
  }

  function refresh() {
    if (stopped) return
    const rows = responses.rows(state.snapshot(context))
    hydrate(rows)
    for (const row of rows) {
      if (row.status === "working") pendingResponses.delete(row.id)
      const response = row.status !== "working" ? responses.take(row.id) : undefined
      if (response) pendingResponses.set(row.id, response)
    }
    for (const id of pendingResponses.keys()) if (!rows.some((row) => row.id === id)) pendingResponses.delete(id)
    const next = JSON.stringify({ rows }) + "\n"
    if (last === next && !pendingResponses.size) return
    last = next
    pending = rows
    drain()
  }

  input.on("line", (line) => {
    const id = readActivation(line)
    if (id === undefined || stopped) return
    activate(context, id, state.snapshot(context))
    refresh()
  })
  child.stdin.on("drain", () => { writable = true; drain() })
  child.stdin.on("error", stop)
  child.on("error", stop)
  child.on("exit", stop)
  unsubscribe = context.data.listen(({ details }) => { state.event(context, details); responses.event(details); refresh() })
  // UI-only changes (tab order/focus/closure) need no server event. Read their current state too.
  timer = setInterval(refresh, interval)
  timer.unref()
  refresh()
  return stop
}
