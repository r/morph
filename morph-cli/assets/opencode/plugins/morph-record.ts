/**
 * Morph session recording plugin for OpenCode.
 *
 * On `session.idle`, fetches ALL session messages via the SDK client and
 * records them as a Morph Run + Trace. Every message (user prompts,
 * assistant responses, tool calls, tool results) is preserved as a
 * separate trace event.
 *
 * Blocks the agent from calling `morph_record_session` via MCP so that
 * no recording artifacts appear in the UI.
 */

import { appendFileSync, mkdirSync, writeFileSync } from "fs"
import { join } from "path"

const recordedSessions = new Set<string>()
let eventSampleCount = 0
const MAX_EVENT_SAMPLES = 30

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

function extractPartsText(parts: any[]): string {
  if (!Array.isArray(parts)) return ""
  return parts
    .filter((p: any) => p.type === "text")
    .map((p: any) => p.text || p.content || "")
    .filter(Boolean)
    .join("\n")
}

function extractText(content: unknown): string {
  if (typeof content === "string") return content
  if (Array.isArray(content)) return extractPartsText(content)
  return ""
}

interface Message {
  role: string
  content: string
}

function extractAllMessages(rawMessages: any[]): Message[] {
  const messages: Message[] = []
  for (const entry of rawMessages) {
    const info = entry.info || entry
    const role = info.role || "unknown"
    const parts = entry.parts || info.parts || []
    const text = extractPartsText(parts) || extractText(info.content)
    if (text) {
      messages.push({ role, content: text })
    }
  }
  return messages
}

async function fetchAllMessages(
  client: any,
  sessionId: string,
  directory: string,
): Promise<Message[]> {
  const resp = await client.session.messages({ path: { id: sessionId } })
  const raw: any[] = resp?.data ?? resp ?? []
  appendLog(directory, `fetched ${raw.length} raw messages for ${sessionId}`)
  return extractAllMessages(raw)
}

async function recordConversation(
  $: any,
  directory: string,
  sessionId: string,
  messages: Message[],
): Promise<boolean> {
  const turnKey = `${sessionId}:${messages.length}:${messages[0]?.content?.slice(0, 40) || ""}`
  if (recordedSessions.has(turnKey)) {
    appendLog(directory, "skipping — already recorded this turn")
    return false
  }

  writeDebug(directory, "last-record.json", {
    timestamp: new Date().toISOString(),
    sessionId,
    messageCount: messages.length,
    roles: messages.map((m) => m.role),
  })

  const messagesJson = JSON.stringify(messages)
  await $`morph run record-session --messages ${messagesJson}`
    .cwd(directory)
    .quiet()

  recordedSessions.add(turnKey)
  appendLog(
    directory,
    `recorded conversation (${messages.length} messages, ${messages.map((m) => m.role).join(",")})`,
  )
  return true
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
  appendLog(directory, "plugin loaded (v6 — SDK capture, tool blocked)")

  return {
    "tool.execute.before": async (input: any, output: any) => {
      if (
        input.tool === "morph_record_session" ||
        input.tool === "mcp_morph_record_session"
      ) {
        throw new Error(
          "Recording is handled automatically by the Morph plugin.",
        )
      }
    },

    event: async ({ event }: { event: any }) => {
      if (eventSampleCount < MAX_EVENT_SAMPLES) {
        eventSampleCount++
        const propKeys = event.properties
          ? Object.keys(event.properties)
          : []
        appendLog(
          directory,
          `event[${eventSampleCount}]: ${event.type} propKeys=[${propKeys.join(",")}]`,
        )
      }

      if (event.type !== "session.idle") return

      const sessionId =
        event.properties?.session?.id ||
        event.properties?.sessionID ||
        event.properties?.session_id ||
        (event as any).session_id ||
        (event as any).sessionID

      if (!sessionId) {
        appendLog(directory, "session.idle: no session ID")
        return
      }

      if (recordedSessions.has(`${sessionId}:`)) return

      appendLog(directory, `session.idle: recording for ${sessionId}`)

      try {
        const messages = await fetchAllMessages(client, sessionId, directory)
        if (messages.length > 0) {
          await recordConversation($, directory, sessionId, messages)
        } else {
          appendLog(directory, "session.idle: no messages found")
        }
      } catch (err: any) {
        appendLog(directory, `session.idle error: ${err?.message || err}`)
      }
    },
  }
}
