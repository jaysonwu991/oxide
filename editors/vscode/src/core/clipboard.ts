// The CLI's `clipboard` command, the read a paste falls back to when the
// webview could not read a copied file itself.
//
// macOS keeps the Desktop, Documents and Downloads folders behind a per-app
// grant, so a webview's read of a copied file there is refused and the paste
// has no bytes to send. The CLI reads the system clipboard the same way its own
// Ctrl+V does — including the pasteboard's own picture when the file cannot be
// read — and answers with the data URL the host writes to a file for
// `--image`, so the panel attaches what the terminal would.

/// The arguments that read the system clipboard as JSON.
export function clipboardArgs(): string[] {
  return ["clipboard", "--json"];
}

export interface ClipboardMedia {
  /// The name the attachment is shown by.
  name: string;
  /// A `data:` URL the CLI built from the clipboard's bytes.
  dataUrl: string;
}

/// Parses `oxide clipboard --json`: `{"name":…,"dataUrl":…}` for an image or a
/// PDF, or `null` for a text paste or an empty clipboard. Anything else is
/// `null` rather than a half-read attachment.
export function parseClipboardMedia(stdout: string): ClipboardMedia | null {
  let value: unknown;
  try {
    value = JSON.parse(stdout);
  } catch {
    return null;
  }
  if (!value || typeof value !== "object") return null;
  const { name, dataUrl } = value as { name?: unknown; dataUrl?: unknown };
  if (typeof name !== "string" || typeof dataUrl !== "string" || !dataUrl.startsWith("data:")) {
    return null;
  }
  return { name, dataUrl };
}
