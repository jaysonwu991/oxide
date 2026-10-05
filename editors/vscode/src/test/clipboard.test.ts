// The `/clipboard` read: what a paste the webview could not read itself falls
// back to, answered by the CLI so the panel attaches what the terminal would.

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { clipboardArgs, parseClipboardMedia } from "../core/clipboard";

describe("clipboard", () => {
  it("asks the CLI for the clipboard as JSON", () => {
    assert.deepEqual(clipboardArgs(), ["clipboard", "--json"]);
  });

  it("reads the attachment the CLI answered with", () => {
    assert.deepEqual(
      parseClipboardMedia('{"name":"shot.png","dataUrl":"data:image/png;base64,QUJD"}'),
      { name: "shot.png", dataUrl: "data:image/png;base64,QUJD" },
    );
  });

  it("reads a null answer as nothing to attach", () => {
    assert.equal(parseClipboardMedia("null"), null);
  });

  it("refuses a half-read or malformed answer", () => {
    // A name with no data URL, a data URL that is not one, and output that is
    // not JSON at all are all nothing to attach rather than a chip with
    // nothing behind it.
    assert.equal(parseClipboardMedia('{"name":"shot.png"}'), null);
    assert.equal(parseClipboardMedia('{"name":"shot.png","dataUrl":"shot.png"}'), null);
    assert.equal(parseClipboardMedia("not json"), null);
    assert.equal(parseClipboardMedia(""), null);
    assert.equal(parseClipboardMedia('"data:image/png;base64,QUJD"'), null);
  });
});
