"use strict";
// Parsing for `oxide sessions list` and `oxide --version`.
//
// The session picker reads the CLI's own listing rather than walking the
// session tree itself, so the extension never has to agree with the core about
// how a project's session directory is named.
Object.defineProperty(exports, "__esModule", { value: true });
exports.parseSessionList = parseSessionList;
exports.parseVersion = parseVersion;
/// One line of `oxide sessions list`:
///
/// ```text
/// 7c8031b1  just now       2 msg  say hi
/// ```
///
/// The label is free text and is always last, so the rest of the line is
/// matched and the label kept whole. Unrecognized lines (the "no sessions"
/// notice, or a future format) are skipped instead of mis-parsed.
const SESSION_LINE = /^([0-9a-fA-F-]+)\s+(just now|\d+(?:m|h|d) ago)\s+(\d+) msg\s*(.*)$/;
function parseSessionList(output) {
    const entries = [];
    for (const line of output.split("\n")) {
        const match = SESSION_LINE.exec(line.trimEnd());
        if (!match)
            continue;
        entries.push({
            id: match[1],
            age: match[2],
            messages: Number(match[3]),
            label: match[4].trim(),
        });
    }
    return entries;
}
/// The version from `oxide --version` (`oxide 0.1.2`).
function parseVersion(output) {
    const match = /(\d+\.\d+\.\d+(?:[-+][^\s]+)?)/.exec(output);
    return match ? match[1] : null;
}
//# sourceMappingURL=sessions.js.map