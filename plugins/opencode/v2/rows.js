function visibleTabs(context) {
  if (context.ui.tabs.enabled()) return context.ui.tabs.list()
  const route = context.ui.router.current()
  if (route.type !== "session") return []
  return [{ sessionID: context.data.session.root(route.sessionID), active: true }]
}

function familyState(context, tab) {
  const data = context.data.session
  const family = [...new Set([tab.sessionID, ...data.family(tab.sessionID)])]
  if (family.some((id) => data.permission.list(id)?.length)) {
    return { status: "blocked", reason: "Permission required" }
  }
  if (family.some((id) => data.form.list(id, data.get(id)?.location)?.length)) {
    return { status: "blocked", reason: "Input required" }
  }
  if (data.form.list("global", data.get(tab.sessionID)?.location)?.length) {
    return { status: "blocked", reason: "Input required" }
  }
  if (tab.busy || family.some((id) => data.status(id) === "running")) {
    return { status: "working" }
  }
  return { status: "idle" }
}

function sessionTitle(session, tab) {
  const title = (session.title ?? tab.title ?? "").trim()
  return title.startsWith("New session - ") ? "" : title
}

export class SessionRows {
  #done = new Set()
  #working = new Set()

  event(context, event) {
    if (event.type !== "session.execution.succeeded") return
    const id = event.data?.sessionID
    if (typeof id === "string") this.#done.add(context.data.session.root(id))
  }

  snapshot(context) {
    const rows = visibleTabs(context).flatMap((tab) => this.#row(context, tab))
    const ids = new Set(rows.map((row) => row.id))
    for (const id of this.#done) if (!ids.has(id)) this.#done.delete(id)
    for (const id of this.#working) if (!ids.has(id)) this.#working.delete(id)
    return rows
  }

  #row(context, tab) {
    const session = context.data.session.get(tab.sessionID)
    // A stale/deleted tab must not publish a conversation that no longer exists.
    if (!session) return []
    const state = familyState(context, tab)
    if (state.status === "working") this.#working.add(tab.sessionID)
    else if (state.status === "idle" && this.#working.delete(tab.sessionID)) this.#done.add(tab.sessionID)
    if (tab.active) this.#done.delete(tab.sessionID)
    if (state.status === "idle" && this.#done.has(tab.sessionID)) state.status = "done"
    return [{
      id: session.id,
      title: sessionTitle(session, tab),
      ...state,
      active: Boolean(tab.active),
      native_session: session.id,
      cwd: session.location.directory,
    }]
  }
}

export function activate(context, id, rows) {
  if (!rows.some((row) => row.id === id)) return
  if (!context.data.session.get(id)) return
  if (context.ui.tabs.enabled()) context.ui.tabs.focus(id)
  else context.ui.router.navigate({ type: "session", sessionID: id })
}
