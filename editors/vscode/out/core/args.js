"use strict";
// CLI argv for one agent turn. Every flag maps onto an `oxide` option, so the
// extension never needs its own copy of the agent's configuration.
Object.defineProperty(exports, "__esModule", { value: true });
exports.buildTurnArgs = buildTurnArgs;
exports.sessionsListArgs = sessionsListArgs;
exports.splitList = splitList;
/// `--mode json` streams one Pi-shaped JSON event per line, and `-p` makes the
/// prompt explicit so it can be delivered on stdin. Sending the prompt on
/// stdin (rather than as a positional argument) keeps the message out of the
/// shell and out of argv parsing; a prompt that is empty is never started.
/// The `@path` references a message may carry are expanded by the extension
/// itself (`core/prompt.ts`), so the composer behaves like the CLI's own
/// positional `@file` handling.
function buildTurnArgs(options) {
    const args = ["--mode", "json", "-p"];
    if (options.session)
        args.push("--session", options.session);
    else if (options.continueLast)
        args.push("--continue");
    if (options.ephemeral)
        args.push("--no-session");
    if (options.model)
        args.push("--model", options.model);
    if (options.agent)
        args.push("--agent", options.agent);
    if (options.reasoning && options.reasoning !== "auto") {
        args.push("--reasoning", options.reasoning);
    }
    if (options.trust === "always")
        args.push("--approve");
    else if (options.trust === "never")
        args.push("--no-approve");
    if (options.tools)
        args.push("--tools", options.tools);
    if (options.excludeTools)
        args.push("--exclude-tools", options.excludeTools);
    for (const path of options.attachments ?? [])
        args.push("--image", path);
    for (const arg of options.extra ?? []) {
        // An empty or whitespace-only entry would be an argument clap rejects.
        if (arg.trim())
            args.push(arg);
    }
    return args;
}
/// `oxide sessions list` for the current project.
function sessionsListArgs() {
    return ["sessions", "list"];
}
/// Splits a comma-separated tool list the way `--tools` expects, dropping the
/// empty entries a trailing comma leaves behind.
function splitList(value) {
    return value
        .split(",")
        .map((entry) => entry.trim())
        .filter(Boolean)
        .join(",");
}
//# sourceMappingURL=args.js.map