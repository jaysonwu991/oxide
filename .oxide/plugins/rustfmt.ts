export const RustFmt = async ({ $, directory }) => {
  return {
    "tool.execute.after": async (input) => {
      const tool = input.tool
      if (tool !== "edit" && tool !== "write" && tool !== "write_file") return

      const args = input.args ?? {}
      const filePath = args.filePath ?? args.path
      if (typeof filePath !== "string" || !filePath.endsWith(".rs")) return

      await $`rustfmt --edition 2021 ${filePath}`.cwd(directory).quiet().nothrow()
    },
  }
}
