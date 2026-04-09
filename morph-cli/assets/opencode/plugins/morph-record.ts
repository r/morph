/**
 * Morph session recording plugin for OpenCode.
 *
 * Uses the `stop` hook (awaited — blocks the agent from exiting) to fetch
 * the session's messages via the SDK client and record them as a Morph
 * Run + Trace. Falls back to `session.idle` if the stop hook doesn't fire.
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

async function fetchSessionMessages(
  client: any,
  sessionId: string,
  directory: string,
): Promise<{ prompt: string; response: string }> {
  let prompt = ""
  let response = ""

  const resp = await client.session.messages({ path: { id: sessionId } })
  const messages: any[] = resp?.data ?? resp ?? []
  appendLog(directory, `fetched ${messages.length} messages for ${sessionId}`)

  for (const entry of messages) {
    const info = entry.info || entry
    const role = info.role
    const parts = entry.parts || info.parts || []
    const text = extractPartsText(parts) || extractText(info.content)

    if (role === "user" && text) prompt = text
    if (role === "assistant" && text) response = text
  }

  return { prompt, response }
}

async function recordSession(
  $: any,
  directory: string,
  sessionId: string,
  prompt: string,
  response: string,
): Promise<boolean> {
  const turnKey = `${sessionId}:${prompt.slice(0, 80)}`
  if (recordedSessions.has(turnKey)) {
    appendLog(directory, "skipping — already recorded this turn")
    return false
  }

  writeDebug(directory, "last-record.json", {
    timestamp: new Date().toISOString(),
    sessionId,
    promptLength: prompt.length,
    responseLength: response.length,
  })

  await $`morph run record-session --prompt ${prompt} --response ${response}`.cwd(
    directory,
  )

  recordedSessions.add(turnKey)
  appendLog(directory, `recorded session (${prompt.length}+${response.length} chars)`)
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
  appendLog(directory, "plugin loaded (v3 — stop hook)")

  return {
    // Track if the agent already recorded via MCP (avoid double-recording)
    "tool.execute.after": async (input: any) => {
      if (
        input.tool === "morph_record_session" ||
        input.tool === "mcp_morph_record_session"
      ) {
        agentRecordedThisTurn = true
        appendLog(directory, "agent called morph_record_session — will skip plugin recording")
      }
    },

    // Primary: stop hook is awaited, so recording completes before exit
    stop: async (input: any) => {
      const sessionId =
        input.sessionID || input.session_id || (input as any).id

      appendLog(directory, `stop hook fired (session=${sessionId || "unknown"})`)

      if (agentRecordedThisTurn) {
        appendLog(directory, "agent already recorded — skipping")
        agentRecordedThisTurn = false
        return
      }

      if (!sessionId) {
        appendLog(directory, "stop hook: no session ID, trying session.list")
        try {
          const sessResp = await client.session.list()
          const sessions: any[] = sessResp?.data ?? sessResp ?? []
          if (sessions.length > 0) {
            const sid = sessions[0].id || sessions[0].sessionID
            if (sid) {
              const { prompt, response } = await fetchSessionMessages(
                client,
                sid,
                directory,
              )
              if (prompt || response) {
                await recordSession($, directory, sid, prompt, response)
              }
            }
          }
        } catch (err: any) {
          appendLog(directory, `stop fallback failed: ${err?.message || err}`)
        }
        agentRecordedThisTurn = false
        return
      }

      try {
        const { prompt, response } = await fetchSessionMessages(
          client,
          sessionId,
          directory,
        )
        if (prompt || response) {
          await recordSession($, directory, sessionId, prompt, response)
        } else {
          appendLog(directory, "stop hook: no prompt/response in session messages")
        }
      } catch (err: any) {
        appendLog(directory, `stop hook error: ${err?.message || err}`)
      }

      agentRecordedThisTurn = false
    },

    // Fallback: session.idle is fire-and-forget but catches edge cases
    event: async ({ event }: { event: any }) => {
      // Diagnostic: log early events so we can debug payload shapes
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

      if (event.type !== "session.idle") return

      // session.idle is a backup — the stop hook should have handled it
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

      appendLog(directory, `session.idle fallback for ${sessionId}`)

      try {
        const { prompt, response } = await fetchSessionMessages(
          client,
          sessionId,
          directory,
        )
        if (prompt || response) {
          await recordSession($, directory, sessionId, prompt, response)
        }
      } catch (err: any) {
        appendLog(directory, `session.idle error: ${err?.message || err}`)
      }
    },
  }
}
