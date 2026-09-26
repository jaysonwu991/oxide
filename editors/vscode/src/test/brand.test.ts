import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { describe, it } from "node:test";

/// The extension package (this file compiles to `out/test/`) and the repository
/// root above it. The brand assets are shared with the desktop app, so the test
/// reaches out of the package to compare against the desktop's checked-in icon.
const root = path.join(__dirname, "..", "..");
const repo = path.join(root, "..", "..");
const desktopIcon = path.join(repo, "crates", "desktop", "icons", "128x128.png");

interface Manifest {
  icon?: string;
  contributes: {
    viewsContainers: Record<string, { id: string; title: string; icon?: string }[]>;
    views: Record<string, { id: string; name: string; icon?: string }[]>;
  };
}

const manifest = JSON.parse(fs.readFileSync(path.join(root, "package.json"), "utf8")) as Manifest;
const icon = manifest.icon ?? "";
const iconFile = path.join(root, icon);
const mark = path.join(root, "media", "oxide.svg");

/// The desktop app's mark: a cyan diamond with a dark rim, drawn as
/// `rgb(95, 215, 255)` on `rgb(45, 45, 58)`. The container icon carries its own
/// colours because VS Code draws a contributed icon as a plain background
/// image, where `currentColor` in a standalone SVG is black — invisible on a
/// dark side bar.
const MARK_COLORS = ["#5fd7ff", "#2d2d3a"];

/// The first bytes of a PNG, followed by the IHDR width and height the icon must
/// have (square, at least the 128x128 VS Code asks for).
const PNG_SIGNATURE = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

describe("brand assets", () => {
  it("shows the desktop app's icon in the extensions view", () => {
    assert.ok(icon.endsWith(".png"), "package.json points at a PNG icon");
    assert.ok(fs.existsSync(iconFile), `${icon} exists`);
    const png = fs.readFileSync(iconFile);
    assert.ok(png.subarray(0, 8).equals(PNG_SIGNATURE), `${icon} is a PNG`);
    const width = png.readUInt32BE(16);
    const height = png.readUInt32BE(20);
    assert.equal(width, height, `${icon} is square`);
    assert.ok(width >= 128, `${icon} is ${width}px, at least the 128 VS Code asks for`);
  });

  it("ships the icon the desktop app builds with", () => {
    assert.ok(fs.existsSync(desktopIcon), "the desktop app icon is checked in");
    const expected = fs.readFileSync(desktopIcon);
    const packaged = fs.readFileSync(iconFile);
    assert.equal(
      packaged.equals(expected),
      true,
      `${path.relative(root, iconFile)} is a copy of ${path.relative(repo, desktopIcon)}`,
    );
  });

  it("draws the container and view icon with the desktop app's mark", () => {
    assert.ok(fs.existsSync(mark), "media/oxide.svg exists");
    const svg = fs.readFileSync(mark, "utf8").toLowerCase();
    assert.equal(svg.includes("currentcolor"), false, "the mark carries its own colours");
    for (const color of MARK_COLORS) {
      assert.ok(svg.includes(color), `the mark uses ${color}`);
    }
  });

  it("offers the mark wherever a chat pane is contributed", () => {
    const { viewsContainers, views } = manifest.contributes;
    const icons = [...Object.values(viewsContainers), ...Object.values(views)].flat();
    assert.ok(icons.length >= 4, "the chat is contributed in both containers");
    for (const entry of icons) {
      assert.equal(entry.icon, "media/oxide.svg", `${entry.id} uses the desktop mark`);
    }
  });
});
