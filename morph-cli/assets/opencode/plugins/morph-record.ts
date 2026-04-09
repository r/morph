/**
 * Morph session recording plugin for OpenCode.
 *
 * Tracks prompts and responses via the generic event handler. When the session
 * goes idle, calls `morph run record-session` to persist the turn as a Morph
 * Run + Trace.
 *
 * This provides always-on recording without relying on the agent to call
 * morph_record_session via MCP.
 */

import { writeFileSync, appendFileSync, mkdirSync } from "fs"
import { join } from "path"

let pendingPrompt = ""
let pendingResponse = ""

function appendLog(directory: string, line: string) {
  try {
    const logDir = join(directory, ".morph", "hooks", "logs")
    mkdirSync(logDir, { recursive: true })
    appendFileSync(
      join(logDir, "opencode-plugin.log"),
      `${new Date().toISOString()} ${line}\n`,
    )
  } catch {}
}

function writeDebug(directory: string, filename: string, data: unknown) {
  try {
    const debugDir = join(directory, ".morph", "hooks", "debug")
    mkdirSync(debugDir, { recursive: true })
    writeFileSync(join(debugDir, filename), JSON.stringify(data, null, 2))
  } catch {}
}

export const MorphRecordPlugin = async ({
  $,
  client,
  directory,
}: {
  $: any
  client: any
  directory: string
}) => {
  appendLog(directory, "plugin loaded")

  return {
    event: async ({ event }: { event: any }) => {
      if (event.type === "message.updated") {
        const msg = event.properties?.message
        if (!msg) return

        if (msg.role === "user" && msg.content) {
          const text =
            typeof msg.content === "string"
              ? msg.content
              : Array.isArray(msg.content)
                ? msg.content
                    .filter((p: any) => p.type === "text")
                    .map((p: any) => p.text)
                    .join("\n")
                : ""
          if (text) {
            pendingPrompt = text
            appendLog(directory, "captured prompt")
          }
        }

        if (msg.role === "assistant" && msg.content) {
          const text =
            typeof msg.content === "string"
              ? msg.content
              : Array.isArray(msg.content)
                ? msg.content
                    .filter((p: any) => p.type === "text")
                    .map((p: any) => p.text)
                    .join("\n")
                : ""
          if (text) {
            pendingResponse = text
            appendLog(directory, "captured response")
          }
        }

        return
      }

      if (event.type !== "session.idle") return
      if (!pendingPrompt && !pendingResponse) {
        appendLog(
          directory,
          "session.idle fired but no pending prompt/response — skipped",
        )
        return
      }

      const prompt = pendingPrompt
      const response = pendingResponse
      pendingPrompt = ""
      pendingResponse = ""

      try {
        writeDebug(directory, "last-opencode-idle.json", {
          timestamp: new Date().toISOString(),
          promptLength: prompt.length,
          responseLength: response.length,
        })

        await $`morph run record-session --prompt ${prompt} --response ${response}`.cwd(
          directory,
        )

        appendLog(directory, "recorded session via CLI")
      } catch (err: any) {
        const errMsg = err?.message || String(err)
        appendLog(directory, `error recording session: ${errMsg}`)
        try {
          await client?.app?.log?.({
            body: {
              service: "morph-record",
              level: "error",
              message: `Failed to record Morph session: ${errMsg}`,
            },
          })
        } catch {}
      }
    },
  }
}
