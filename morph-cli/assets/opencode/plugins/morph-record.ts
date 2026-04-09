/**
 * Morph session recording plugin for OpenCode.
 *
 * OpenCode delivers message metadata and content through separate events:
 *   - `message.updated` carries role + message ID (no content).
 *   - `message.part.updated` carries the actual text, keyed by messageID.
 *   - `message.part.delta` streams incremental text.
 *
 * On `session.idle` we assemble the latest user prompt and assistant
 * response and call `morph run record-session`.
 *
 * If the agent already called `morph_record_session` via MCP during the
 * turn, we skip to avoid double-recording.
 */

import { appendFileSync, mkdirSync, writeFileSync } from "fs"
import { join } from "path"

interface MessageInfo {
  role: string
  text: string
}

const messages = new Map<string, MessageInfo>()
const recordedTurns = new Set<string>()
let agentRecordedThisTurn = false

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

function latestByRole(role: string): string {
  let latest = ""
  for (const m of messages.values()) {
    if (m.role === role && m.text) latest = m.text
  }
  return latest
}

export const MorphRecordPlugin = async ({
  $,
  directory,
}: {
  $: any
  directory: string
}) => {
  appendLog(directory, "plugin loaded (v5 — part-based capture)")

  return {
    "tool.execute.after": async (input: any) => {
      if (
        input.tool === "morph_record_session" ||
        input.tool === "mcp_morph_record_session"
      ) {
        agentRecordedThisTurn = true
        appendLog(
          directory,
          "agent called morph_record_session — will skip plugin recording",
        )
      }
    },

    event: async ({ event }: { event: any }) => {
      if (event.type === "message.updated") {
        const info = event.properties?.info
        if (!info?.id || !info?.role) return
        const existing = messages.get(info.id)
        if (!existing) {
          messages.set(info.id, { role: info.role, text: "" })
        }
        return
      }

      if (event.type === "message.part.updated") {
        const part = event.properties?.part
        if (!part?.messageID) return
        if (part.type !== "text") return

        const text = part.text || ""
        if (!text) return

        const msg = messages.get(part.messageID)
        if (msg) {
          msg.text = text
        } else {
          messages.set(part.messageID, { role: "unknown", text })
        }
        return
      }

      if (event.type !== "session.idle") return

      if (agentRecordedThisTurn) {
        appendLog(directory, "session.idle: agent already recorded — skipping")
        agentRecordedThisTurn = false
        messages.clear()
        return
      }

      const prompt = latestByRole("user")
      const response = latestByRole("assistant")

      if (!prompt && !response) {
        appendLog(directory, "session.idle: no prompt/response captured — skipped")
        return
      }

      const turnKey = prompt.slice(0, 120)
      if (recordedTurns.has(turnKey)) {
        appendLog(directory, "session.idle: already recorded this turn — skipped")
        messages.clear()
        return
      }

      writeDebug(directory, "last-record.json", {
        timestamp: new Date().toISOString(),
        promptLength: prompt.length,
        responseLength: response.length,
        messageCount: messages.size,
      })

      try {
        await $`morph run record-session --prompt ${prompt} --response ${response}`
          .cwd(directory)
          .quiet()
        recordedTurns.add(turnKey)
        appendLog(
          directory,
          `session.idle: recorded (${prompt.length}+${response.length} chars)`,
        )
      } catch (err: any) {
        appendLog(
          directory,
          `session.idle: error recording: ${err?.message || err}`,
        )
      }

      messages.clear()
      agentRecordedThisTurn = false
    },
  }
}
