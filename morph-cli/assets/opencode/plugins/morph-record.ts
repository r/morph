/**
 * Morph session recording plugin for OpenCode.
 *
 * Tracks prompts and responses via message events. When the session goes idle,
 * calls `morph run record-session` to persist the turn as a Morph Run + Trace.
 *
 * This provides always-on recording without relying on the agent to call
 * morph_record_session via MCP.
 */

let pendingPrompt = ""
let pendingResponse = ""

export const MorphRecordPlugin = async ({
  $,
  directory,
}: {
  $: any
  directory: string
}) => {
  return {
    "message.updated": async ({ event }: { event: any }) => {
      const msg = event.properties
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
        if (text) pendingPrompt = text
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
        if (text) pendingResponse = text
      }
    },

    event: async ({ event }: { event: any }) => {
      if (event.type !== "session.idle") return
      if (!pendingPrompt && !pendingResponse) return

      const prompt = pendingPrompt
      const response = pendingResponse
      pendingPrompt = ""
      pendingResponse = ""

      try {
        await $`morph run record-session --prompt ${prompt} --response ${response}`.cwd(
          directory,
        )
      } catch {
        // Best-effort; agent-driven recording via MCP is the fallback.
      }
    },
  }
}
