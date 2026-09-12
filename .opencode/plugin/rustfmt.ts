import type { Plugin } from "@opencode-ai/plugin"

export const RustFmt: Plugin = async ({ $, directory }) => {
  return {
    "tool.execute.after": async (input, output) => {
      const tool = input.tool
      if (tool !== "edit" && tool !== "write" && tool !== "write_file") return

      const args = (input as { args?: Record<string, unknown> }).args ?? {}
      const filePath = args.filePath ?? args.path
      if (typeof filePath !== "string" || !filePath.endsWith(".rs")) return

      await $`rustfmt --edition 2021 ${filePath}`.cwd(directory).quiet().nothrow()
    },
  }
}
