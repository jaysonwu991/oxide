// A tolerant JSON object reader. Every shared file the extension peeks at
// (`config.json`, `settings.json`, `trust.json`) belongs to the CLI and may be
// mid-write or hand-edited, so a malformed file has to read as "nothing here"
// rather than throw.

/// Parses `raw` into an object, or `null` when it is missing, malformed, or not
/// an object.
export function parseObject(raw: string | null | undefined): Record<string, unknown> | null {
  if (!raw) return null;
  try {
    const value: unknown = JSON.parse(raw);
    if (!value || typeof value !== "object" || Array.isArray(value)) return null;
    return value as Record<string, unknown>;
  } catch {
    return null;
  }
}
