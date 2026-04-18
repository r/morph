/**
 * Morph session recording plugin for OpenCode.
 *
 * Captures structured trace events from an OpenCode session:
 *   - user / assistant text
 *   - reasoning (thinking) blocks
 *   - file_read / file_edit (for built-in read/edit/write/grep/glob/list/webfetch/apply_patch)
 *   - tool_call / tool_result (for bash, todowrite, task, skill, websearch, MCP servers, ...)
 *   - error events when a tool fails
 *   - usage metadata (tokens/cost) from step-finish parts
 *
 * The primary recording path is `session.idle` → SDK `session.messages()`, which
 * returns each message with its structured `parts` array. OpenCode's `ToolPart`
 * carries the full tool call including `state.input`, `state.output`, `state.error`,
 * timings and metadata — this plugin fans those into distinct Morph trace events
 * so downstream replay / eval tooling can consume them.
 *
 * A `tool.execute.before` / `tool.execute.after` pair is also wired for lightweight
 * real-time observability into the `.morph/hooks/logs/opencode-plugin.log`.
 *
 * Blocks the agent from calling `morph_record_session` via MCP so that no
 * recording artifacts appear in the chat.
 */

import { appendFileSync, mkdirSync, writeFileSync } from "fs"
import { join } from "path"

// ---------- debug / observability helpers ----------

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

// ---------- content limits ----------

const MAX_CONTENT_LEN = 4000
const MAX_TOOL_INPUT_LEN = 4000
const MAX_TOOL_OUTPUT_LEN = 8000

function truncate(s: string, limit = MAX_CONTENT_LEN): string {
  if (s && s.length > limit) return s.slice(0, limit) + "... [truncated]"
  return s
}

function safeJson(v: unknown, limit = MAX_TOOL_INPUT_LEN): string {
  try {
    return truncate(JSON.stringify(v ?? null), limit)
  } catch {
    return truncate(String(v ?? ""), limit)
  }
}

function rfc3339(msOrSec: number | undefined): string | undefined {
  if (typeof msOrSec !== "number" || !isFinite(msOrSec)) return undefined
  // OpenCode timestamps are milliseconds since epoch.
  const ms = msOrSec > 1e12 ? msOrSec : msOrSec * 1000
  try {
    return new Date(ms).toISOString()
  } catch {
    return undefined
  }
}

// ---------- tool classification ----------

// OpenCode built-in tool IDs are lowercase. See https://opencode.ai/docs/tools.
const FILE_READ_TOOLS = new Set([
  "read", "grep", "glob", "list", "webfetch",
])
const FILE_EDIT_TOOLS = new Set([
  "edit", "write", "apply_patch", "multiedit", "patch",
])

type ToolRole = "tool_call" | "file_read" | "file_edit"

function classifyTool(name: string | undefined): ToolRole {
  const n = (name || "").toLowerCase()
  if (FILE_READ_TOOLS.has(n)) return "file_read"
  if (FILE_EDIT_TOOLS.has(n)) return "file_edit"
  return "tool_call"
}

function extractPath(toolName: string, input: Record<string, any>): string | undefined {
  const n = (toolName || "").toLowerCase()
  if (!input || typeof input !== "object") return undefined
  // Common path fields used by OpenCode built-in tools.
  const candidates = [
    input.filePath, input.path, input.file, input.target,
    input.pattern, input.glob,
  ]
  for (const c of candidates) {
    if (typeof c === "string" && c.length > 0) return c
  }
  // apply_patch encodes paths inside patchText marker lines; record the full
  // patch text as content instead and leave path undefined.
  if (n === "apply_patch" || n === "patch") return undefined
  return undefined
}

// ---------- Morph ConversationMessage shape ----------

interface Message {
  role: string
  content: string
  metadata?: Record<string, any>
  timestamp?: string
}

interface FetchResult {
  messages: Message[]
  modelName: string
  counts: Record<string, number>
}

function bump(counts: Record<string, number>, role: string) {
  counts[role] = (counts[role] ?? 0) + 1
}

// ---------- part → messages ----------
//
// Each OpenCode Part becomes one or more Morph events. The mapping is intentionally
// lossless where possible so downstream tools can reconstruct the agent's behaviour.

function partsFromTextLike(role: string, part: any): Message | null {
  const ptype = part.type
  const text = part.text ?? part.content ?? ""
  if (!text) return null
  const metadata: Record<string, any> = {}
  if (ptype === "reasoning") metadata.subkind = "reasoning"
  const ts = rfc3339(part.time?.start ?? part.time?.end)
  return {
    role: ptype === "reasoning" ? "reasoning" : role,
    content: truncate(String(text)),
    metadata: Object.keys(metadata).length ? metadata : undefined,
    timestamp: ts,
  }
}

function partsFromToolPart(part: any): Message[] {
  const toolName: string = part.tool || part.name || "unknown_tool"
  const callId: string | undefined = part.callID || part.id
  const state = part.state ?? {}
  const status: string = state.status || "pending"
  const input: Record<string, any> = (state.input ?? {}) as any

  const role = classifyTool(toolName)
  const path = extractPath(toolName, input)

  const callMeta: Record<string, any> = {
    name: toolName,
    status,
    input: safeJson(input),
  }
  if (callId) callMeta.call_id = callId
  if (path) callMeta.path = path
  if (state.title) callMeta.title = String(state.title)

  // For file edits, also store the new/replacement content so tap can surface
  // it in agentic eval contexts.
  if (role === "file_edit" && typeof input === "object") {
    const newContent =
      input.content ?? input.newString ?? input.new_string ?? input.patchText
    if (typeof newContent === "string") {
      callMeta.new_content = truncate(newContent, MAX_TOOL_INPUT_LEN)
    }
  }

  const callContent =
    role === "file_edit" && typeof callMeta.new_content === "string"
      ? callMeta.new_content
      : role === "file_read" && path
        ? path
        : safeJson(input)

  const events: Message[] = [
    {
      role,
      content: callContent,
      metadata: callMeta,
      timestamp: rfc3339(state.time?.start),
    },
  ]

  if (status === "completed") {
    const output = typeof state.output === "string" ? state.output : safeJson(state.output)
    const resMeta: Record<string, any> = { name: toolName }
    if (callId) resMeta.call_id = callId
    if (path) resMeta.path = path
    events.push({
      role: "tool_result",
      content: truncate(String(output), MAX_TOOL_OUTPUT_LEN),
      metadata: resMeta,
      timestamp: rfc3339(state.time?.end),
    })
  } else if (status === "error") {
    const resMeta: Record<string, any> = {
      name: toolName,
      error: String(state.error ?? "unknown error"),
    }
    if (callId) resMeta.call_id = callId
    events.push({
      role: "error",
      content: truncate(String(state.error ?? ""), MAX_TOOL_OUTPUT_LEN),
      metadata: resMeta,
      timestamp: rfc3339(state.time?.end),
    })
  }

  return events
}

function partsFromFilePart(part: any): Message | null {
  // A FilePart represents a file attachment injected into the message (e.g.
  // @file reference or drag-and-drop). Record it as a file_read so tap sees
  // the path in context.
  const source = part.source ?? {}
  const path = source.path || part.filename || ""
  const content = source.text?.value ?? ""
  if (!path && !content) return null
  return {
    role: "file_read",
    content: truncate(String(content || path)),
    metadata: {
      name: "attach",
      path: path || undefined,
      mime: part.mime || undefined,
    },
  }
}

function partsFromPatchPart(part: any): Message[] {
  const files: string[] = Array.isArray(part.files) ? part.files : []
  return files.map((p: string): Message => ({
    role: "file_edit",
    content: p,
    metadata: { name: "patch", path: p, hash: part.hash },
  }))
}

function partsFromStepFinish(part: any): Message | null {
  const tokens = part.tokens
  if (!tokens) return null
  return {
    role: "usage",
    content: "",
    metadata: {
      reason: part.reason,
      cost: part.cost,
      input_tokens: tokens.input,
      output_tokens: tokens.output,
      reasoning_tokens: tokens.reasoning,
      cache_read_tokens: tokens.cache?.read,
      cache_write_tokens: tokens.cache?.write,
    },
  }
}

function extractAllMessages(rawMessages: any[], directory: string): FetchResult {
  const messages: Message[] = []
  const counts: Record<string, number> = {}
  let modelName = ""
  let toolSamplesDumped = 0
  const sampleParts: any[] = []

  for (const entry of rawMessages) {
    const info = entry.info ?? entry
    const role = info.role || "unknown"
    const parts = entry.parts ?? info.parts ?? []
    if (!modelName && info.modelID) {
      modelName = info.providerID
        ? `${info.providerID}/${info.modelID}`
        : info.modelID
    }

    if (!Array.isArray(parts) || parts.length === 0) {
      // Fallback: some legacy messages may carry a string `content` field
      // without structured parts. Preserve the text.
      const c = (info as any).content
      if (typeof c === "string" && c) {
        messages.push({ role, content: truncate(c) })
        bump(counts, role)
      }
      continue
    }

    for (const part of parts) {
      const ptype = part?.type
      if (toolSamplesDumped < 3 && ptype === "tool") {
        sampleParts.push(part)
        toolSamplesDumped++
      }

      if (ptype === "text") {
        const m = partsFromTextLike(role, part)
        if (m) { messages.push(m); bump(counts, m.role) }
      } else if (ptype === "reasoning") {
        const m = partsFromTextLike(role, part)
        if (m) { messages.push(m); bump(counts, m.role) }
      } else if (ptype === "tool") {
        const ms = partsFromToolPart(part)
        for (const m of ms) { messages.push(m); bump(counts, m.role) }
      } else if (ptype === "file") {
        const m = partsFromFilePart(part)
        if (m) { messages.push(m); bump(counts, m.role) }
      } else if (ptype === "patch") {
        const ms = partsFromPatchPart(part)
        for (const m of ms) { messages.push(m); bump(counts, m.role) }
      } else if (ptype === "step-finish") {
        const m = partsFromStepFinish(part)
        if (m) { messages.push(m); bump(counts, m.role) }
      }
      // Silently skip step-start, snapshot, agent, retry, compaction, subtask.
    }
  }

  if (sampleParts.length) {
    writeDebug(directory, "last-tool-parts.json", sampleParts)
  }

  return { messages, modelName, counts }
}

async function fetchAllMessages(
  client: any,
  sessionId: string,
  directory: string,
): Promise<FetchResult> {
  const resp = await client.session.messages({ path: { id: sessionId } })
  const raw: any[] = resp?.data ?? resp ?? []
  appendLog(directory, `fetched ${raw.length} raw messages for ${sessionId}`)
  return extractAllMessages(raw, directory)
}

async function recordConversation(
  $: any,
  directory: string,
  sessionId: string,
  messages: Message[],
  modelName: string,
  counts: Record<string, number>,
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
    counts,
    modelName,
  })

  const structuredKinds = [
    "tool_call", "tool_result", "file_read", "file_edit", "error", "reasoning",
  ]
  const structuredTotal = structuredKinds.reduce((s, k) => s + (counts[k] ?? 0), 0)
  if (structuredTotal === 0) {
    appendLog(
      directory,
      `WARN: no structured events captured for ${sessionId} — only ${JSON.stringify(counts)} — ` +
        "trace will fall back to plain user/assistant text",
    )
  }

  const messagesJson = JSON.stringify(messages)
  const model = modelName || "unknown"
  await $`morph run record-session --messages ${messagesJson} --model-name ${model} --agent-id opencode`
    .cwd(directory)
    .quiet()

  recordedSessions.add(turnKey)
  appendLog(
    directory,
    `recorded conversation (${messages.length} messages, model=${model}, counts=${JSON.stringify(counts)})`,
  )
  return true
}

// ---------- plugin entry ----------

export const MorphRecordPlugin = async ({
  $,
  client,
  directory,
}: {
  $: any
  client: any
  directory: string
}) => {
  appendLog(directory, "plugin loaded (v7 — structured parts: text/reasoning/tool/file/patch)")

  return {
    "tool.execute.before": async (input: any, _output: any) => {
      if (
        input.tool === "morph_record_session" ||
        input.tool === "mcp_morph_record_session"
      ) {
        throw new Error(
          "Recording is handled automatically by the Morph plugin.",
        )
      }
      // Lightweight observability log. The authoritative capture still comes
      // from the session.idle SDK fetch below.
      appendLog(
        directory,
        `tool.before ${input.tool} call=${input.callID ?? "?"} session=${input.sessionID ?? "?"}`,
      )
    },

    "tool.execute.after": async (input: any, output: any) => {
      const outLen =
        typeof output?.output === "string"
          ? output.output.length
          : JSON.stringify(output?.output ?? "").length
      appendLog(
        directory,
        `tool.after  ${input.tool} call=${input.callID ?? "?"} out_len=${outLen}`,
      )
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
            result.counts,
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
