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
  await publish("idle")

  return {
    event: async ({ event }) => {
      switch (event.type) {
        case "session.status":
          if (event.properties.status.type === "idle") {
            await publish("idle")
          } else {
            await publish("working")
          }
          break
        case "session.idle":
          await publish("idle")
          break
        case "session.error":
          await publish("blocked", "session error")
          break
        case "permission.asked":
          await publish("blocked", "permission required")
          break
        case "permission.replied":
          await publish("working")
          break
        case "question.asked":
          await publish("blocked", "answer required")
          break
        case "question.replied":
        case "question.rejected":
          await publish("working")
          break
      }
    },
    dispose: async () => publish("idle"),
  }
}
