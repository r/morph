/**
 * Morph session recording plugin for OpenCode.
 *
 * Two capture strategies (whichever fires first wins):
 *   1. `stop` hook — awaited, fetches full session via SDK client.
 *   2. Event-based — captures prompt/response from `message.updated` events,
 *      records on `session.idle`.
 *
 * If the agent already called `morph_record_session` via MCP, we skip to
 * avoid double-recording.
 */

import { appendFileSync, mkdirSync, writeFileSync } from "fs"
import { join } from "path"

const recordedTurns = new Set<string>()
let agentRecordedThisTurn = false

let pendingPrompt = ""
let pendingResponse = ""

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

function extractText(content: unknown): string {
  if (typeof content === "string") return content
  if (Array.isArray(content)) {
    return content
      .filter((p: any) => p.type === "text")
      .map((p: any) => p.text || p.content || "")
      .filter(Boolean)
      .join("\n")
  }
  return ""
}

function turnKey(prompt: string): string {
  return prompt.slice(0, 120)
}

async function doRecord(
  $: any,
  directory: string,
  prompt: string,
  response: string,
  source: string,
): Promise<boolean> {
  const key = turnKey(prompt)
  if (recordedTurns.has(key)) {
    appendLog(directory, `${source}: skipping — already recorded this turn`)
    return false
  }
  if (!prompt && !response) {
    appendLog(directory, `${source}: skipping — empty prompt and response`)
    return false
  }

  writeDebug(directory, "last-record.json", {
    timestamp: new Date().toISOString(),
    source,
    promptLength: prompt.length,
    responseLength: response.length,
  })

  await $`morph run record-session --prompt ${prompt} --response ${response}`
    .cwd(directory)
    .quiet()

  recordedTurns.add(key)
  appendLog(
    directory,
    `${source}: recorded (${prompt.length}+${response.length} chars)`,
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
  appendLog(directory, "plugin loaded (v4 — dual capture)")

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
        appendLog(directory, "stop: agent already recorded — skipping")
        agentRecordedThisTurn = false
        pendingPrompt = ""
        pendingResponse = ""
        return
      }

      // Strategy A: fetch messages via SDK
      if (sessionId && client?.session?.messages) {
        try {
          const resp = await client.session.messages({
            path: { id: sessionId },
          })
          const messages: any[] = resp?.data ?? resp ?? []
          appendLog(
            directory,
            `stop: SDK returned ${messages.length} messages`,
          )

          let prompt = ""
          let response = ""
          for (const entry of messages) {
            const info = entry.info || entry
            const role = info.role
            const parts = entry.parts || info.parts || []
            const text = extractText(parts) || extractText(info.content)
            if (role === "user" && text) prompt = text
            if (role === "assistant" && text) response = text
          }

          if (prompt || response) {
            const ok = await doRecord($, directory, prompt, response, "stop/sdk")
            if (ok) {
              pendingPrompt = ""
              pendingResponse = ""
              agentRecordedThisTurn = false
              return
            }
          } else {
            appendLog(directory, "stop: SDK returned no usable prompt/response")
          }
        } catch (err: any) {
          appendLog(
            directory,
            `stop: SDK fetch failed: ${err?.message || err}`,
          )
        }
      }

      // Strategy B: use event-captured data
      if (pendingPrompt || pendingResponse) {
        appendLog(directory, "stop: falling back to event-captured data")
        await doRecord(
          $,
          directory,
          pendingPrompt,
          pendingResponse,
          "stop/events",
        )
      } else {
        appendLog(directory, "stop: no data from SDK or events")
      }

      pendingPrompt = ""
      pendingResponse = ""
      agentRecordedThisTurn = false
    },

    event: async ({ event }: { event: any }) => {
      // Diagnostic: log early events to debug payload shapes
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
          writeDebug(
            directory,
            `event-${event.type}-${eventSampleCount}.json`,
            { type: event.type, propKeys, sample: safeProps },
          )
        }
      }

      // Capture messages from events (used as fallback in stop hook and session.idle)
      if (event.type === "message.updated") {
        const msg =
          event.properties?.message ||
          event.properties?.info ||
          event.properties
        if (!msg) return

        const role = msg.role
        const text =
          extractText(msg.parts) || extractText(msg.content) || extractText(msg.text)

        if (role === "user" && text) {
          pendingPrompt = text
          appendLog(directory, `event: captured prompt (${text.length} chars)`)
        }
        if (role === "assistant" && text) {
          pendingResponse = text
          appendLog(
            directory,
            `event: captured response (${text.length} chars)`,
          )
        }
        return
      }

      if (event.type === "message.part.updated") {
        const part = event.properties?.part || event.properties
        const role =
          part?.role ||
          event.properties?.message?.role ||
          event.properties?.info?.role
        const text = extractText(part?.content) || extractText(part?.text)

        if (role === "user" && text) {
          pendingPrompt = text
        }
        if (role === "assistant" && text) {
          pendingResponse = text
        }
        return
      }

      // session.idle — fire-and-forget fallback if stop hook didn't fire
      if (event.type !== "session.idle") return

      if (agentRecordedThisTurn) {
        agentRecordedThisTurn = false
        pendingPrompt = ""
        pendingResponse = ""
        return
      }

      if (!pendingPrompt && !pendingResponse) {
        appendLog(
          directory,
          "session.idle: no pending data — skipped",
        )
        return
      }

      appendLog(directory, "session.idle: recording from event-captured data")
      try {
        await doRecord(
          $,
          directory,
          pendingPrompt,
          pendingResponse,
          "idle/events",
        )
      } catch (err: any) {
        appendLog(
          directory,
          `session.idle error: ${err?.message || err}`,
        )
      }

      pendingPrompt = ""
      pendingResponse = ""
      agentRecordedThisTurn = false
    },
  }
}
