import { spawn } from "node:child_process"
import { createInterface } from "node:readline"
import { activate, SessionRows } from "./rows.js"

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
  const child = launch(env.ROZI_BIN, ["publish"], { env, stdio: ["pipe", "pipe", "ignore"] })
  const input = createInterface({ input: child.stdout })
  const state = new SessionRows()
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
    input.close()
    // EOF withdraws this publisher's rows. Kill also covers a stuck bridge during unload.
    child.stdin.end()
    child.kill()
  }

  function drain() {
    if (stopped || !writable || pending === undefined) return
    const line = pending
    pending = undefined
    writable = child.stdin.write(line)
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
    const rows = state.snapshot(context)
    hydrate(rows)
    const next = JSON.stringify({ rows }) + "\n"
    if (last === next) return
    last = next
    pending = next
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
  unsubscribe = context.data.listen(({ details }) => { state.event(context, details); refresh() })
  // UI-only changes (tab order/focus/closure) need no server event. Read their current state too.
  timer = setInterval(refresh, interval)
  timer.unref()
  refresh()
  return stop
}
