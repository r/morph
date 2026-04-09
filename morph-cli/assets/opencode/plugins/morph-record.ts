/**
 * Morph session recording plugin for OpenCode.
 *
 * Uses the `stop` hook (awaited — blocks the agent from exiting) to fetch
 * ALL session messages via the SDK client and record them as a Morph
 * Run + Trace. Every message (user prompts, assistant responses, tool calls,
 * tool results) is preserved as a separate trace event.
 *
 * If the agent already called `morph_record_session` via MCP during the
 * turn, we skip to avoid double-recording.
 */

import { appendFileSync, mkdirSync, writeFileSync } from "fs"
import { join } from "path"

const recordedSessions = new Set<string>()
let agentRecordedThisTurn = false
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
  await $`morph run record-session --messages ${messagesJson}`.cwd(directory)

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
  appendLog(directory, "plugin loaded (v4 — full conversation recording)")

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

    stop: async (input: any) => {
      const sessionId =
        input.sessionID || input.session_id || (input as any).id

      appendLog(
        directory,
        `stop hook fired (session=${sessionId || "unknown"})`,
      )

      if (agentRecordedThisTurn) {
        appendLog(directory, "agent already recorded — skipping")
        agentRecordedThisTurn = false
        return
      }

      let messages: Message[] = []

      // Try fetching messages with the session ID from the stop hook
      if (sessionId && client?.session?.messages) {
        try {
          messages = await fetchAllMessages(client, sessionId, directory)
        } catch (err: any) {
          appendLog(
            directory,
            `SDK fetch failed for ${sessionId}: ${err?.message || err}`,
          )
        }
      }

      // Fallback: find the most recent session
      if (messages.length === 0 && client?.session?.list) {
        try {
          const sessResp = await client.session.list()
          const sessions: any[] = sessResp?.data ?? sessResp ?? []
          if (sessions.length > 0) {
            const sid = sessions[0].id || sessions[0].sessionID
            if (sid) {
              appendLog(directory, `trying latest session ${sid}`)
              messages = await fetchAllMessages(client, sid, directory)
            }
          }
        } catch (err: any) {
          appendLog(
            directory,
            `session list fallback failed: ${err?.message || err}`,
          )
        }
      }

      if (messages.length === 0) {
        appendLog(directory, "stop hook: no messages found")
        agentRecordedThisTurn = false
        return
      }

      try {
        await recordConversation($, directory, sessionId || "unknown", messages)
      } catch (err: any) {
        appendLog(directory, `stop hook error: ${err?.message || err}`)
      }

      agentRecordedThisTurn = false
    },

    event: async ({ event }: { event: any }) => {
      // Diagnostic: log early events
      if (eventSampleCount < MAX_EVENT_SAMPLES) {
        eventSampleCount++
        const propKeys = event.properties
          ? Object.keys(event.properties)
          : []
        appendLog(
          directory,
          `event[${eventSampleCount}]: ${event.type} propKeys=[${propKeys.join(",")}]`,
        )
        if (
          event.type === "message.updated" ||
          event.type === "message.part.updated"
        ) {
          const safeProps: Record<string, unknown> = {}
          for (const [k, v] of Object.entries(event.properties || {})) {
            const s = JSON.stringify(v)
            safeProps[k] = s && s.length > 500 ? `(${s.length} chars)` : v
          }
          writeDebug(directory, `event-${event.type}-${Date.now()}.json`, {
            type: event.type,
            propKeys,
            sample: safeProps,
          })
        }
      }

      // session.idle fallback (fire-and-forget, in case stop hook didn't fire)
      if (event.type !== "session.idle") return

      const sessionId =
        event.properties?.session?.id ||
        event.properties?.sessionID ||
        event.properties?.session_id ||
        (event as any).session_id ||
        (event as any).sessionID

      if (!sessionId || recordedSessions.has(`${sessionId}:`)) return

      appendLog(directory, `session.idle fallback for ${sessionId}`)

      try {
        const messages = await fetchAllMessages(client, sessionId, directory)
        if (messages.length > 0) {
          await recordConversation($, directory, sessionId, messages)
        }
      } catch (err: any) {
        appendLog(directory, `session.idle error: ${err?.message || err}`)
      }
    },
  }
}
