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
  metadata?: Record<string, any>
  timestamp?: string
}

interface FetchResult {
  messages: Message[]
  modelName: string
}

const MAX_CONTENT_LEN = 2000

function truncate(s: string, limit = MAX_CONTENT_LEN): string {
  if (s && s.length > limit) return s.slice(0, limit) + "... [truncated]"
  return s
}

const FILE_READ_TOOLS = new Set(["Read", "Grep", "Glob", "SemanticSearch"])
const FILE_EDIT_TOOLS = new Set(["StrReplace", "Write", "EditNotebook", "Delete"])

function partToMessage(role: string, part: any): Message | null {
  const ptype = part.type || "text"

  if (ptype === "text") {
    const text = part.text || part.content || ""
    if (!text) return null
    return { role, content: text }
  }

  if (ptype === "tool-invocation" || ptype === "tool_use" || ptype === "tool-call") {
    const toolName = part.name || part.toolName || "unknown_tool"
    const toolInput = part.input || part.args || {}
    const metadata: Record<string, any> = { name: toolName }

    let msgRole: string
    if (FILE_READ_TOOLS.has(toolName)) {
      msgRole = "file_read"
      metadata.path = toolInput.path || toolInput.glob_pattern || toolInput.pattern || ""
    } else if (FILE_EDIT_TOOLS.has(toolName)) {
      msgRole = "file_edit"
      metadata.path = toolInput.path || toolInput.target_notebook || ""
    } else {
      msgRole = "tool_call"
      metadata.input = truncate(JSON.stringify(toolInput))
    }

    return { role: msgRole, content: truncate(JSON.stringify(toolInput)), metadata }
  }

  if (ptype === "tool-result" || ptype === "tool_result") {
    const output = typeof part.output === "string"
      ? part.output
      : typeof part.content === "string"
        ? part.content
        : JSON.stringify(part.output || part.content || "")
    return {
      role: "tool_result",
      content: truncate(output),
      metadata: part.error ? { error: String(part.error) } : undefined,
    }
  }

  return null
}

function extractAllMessages(rawMessages: any[]): FetchResult {
  const messages: Message[] = []
  let modelName = ""
  for (const entry of rawMessages) {
    const info = entry.info || entry
    const role = info.role || "unknown"
    const parts = entry.parts || info.parts || []
    if (!modelName && info.modelID) {
      modelName = info.providerID
        ? `${info.providerID}/${info.modelID}`
        : info.modelID
    }

    let emittedFromParts = false
    if (Array.isArray(parts)) {
      for (const part of parts) {
        const msg = partToMessage(role, part)
        if (msg) {
          messages.push(msg)
          emittedFromParts = true
        }
      }
    }

    if (!emittedFromParts) {
      const text = extractText(info.content)
      if (text) {
        messages.push({ role, content: text })
      }
    }
  }
  return { messages, modelName }
}

async function fetchAllMessages(
  client: any,
  sessionId: string,
  directory: string,
): Promise<FetchResult> {
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
  modelName: string,
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
    modelName,
  })

  const messagesJson = JSON.stringify(messages)
  const model = modelName || "unknown"
  await $`morph run record-session --messages ${messagesJson} --model-name ${model} --agent-id opencode`
    .cwd(directory)
    .quiet()

  recordedSessions.add(turnKey)
  appendLog(
    directory,
    `recorded conversation (${messages.length} messages, model=${model})`,
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
        const result = await fetchAllMessages(client, sessionId, directory)
        if (result.messages.length > 0) {
          await recordConversation(
            $,
            directory,
            sessionId,
            result.messages,
            result.modelName,
          )
        } else {
          appendLog(directory, "session.idle: no messages found")
        }
      } catch (err: any) {
        appendLog(directory, `session.idle error: ${err?.message || err}`)
      }
    },
  }
}
