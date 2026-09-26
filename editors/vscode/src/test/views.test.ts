import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { describe, it } from "node:test";

import { CHAT_VIEW, CHAT_VIEW_SECONDARY } from "../core/views";

/// The package root: this file compiles to `out/test/`.
const root = path.join(__dirname, "..", "..");

interface Container {
  id: string;
  title: string;
  icon: string;
}

interface View {
  type: string;
  id: string;
  name: string;
}

interface MenuItem {
  command: string;
  when?: string;
}

interface Manifest {
  activationEvents: string[];
  contributes: {
    viewsContainers: { activitybar?: Container[]; secondarySidebar?: Container[] };
    views: Record<string, View[]>;
    menus: { "view/title"?: MenuItem[] };
  };
}

const manifest = JSON.parse(fs.readFileSync(path.join(root, "package.json"), "utf8")) as Manifest;
const { activitybar = [], secondarySidebar = [] } = manifest.contributes.viewsContainers;

/// The view ids contributed inside one container, in order.
const viewsIn = (containerId: string): string[] =>
  (manifest.contributes.views[containerId] ?? []).map((view) => view.id);

/// The two panes the extension host registers a provider for. A view VS Code
/// contributes without a provider — or a provider registered for a view the
/// manifest does not contribute — is an empty panel, which reads as "there is
/// no chat".
const panes = [CHAT_VIEW, CHAT_VIEW_SECONDARY];

describe("view contributions", () => {
  it("contributes one chat view per container", () => {
    assert.equal(activitybar.length, 1);
    assert.equal(secondarySidebar.length, 1);
    assert.deepEqual(viewsIn(activitybar[0].id), [CHAT_VIEW]);
    assert.deepEqual(viewsIn(secondarySidebar[0].id), [CHAT_VIEW_SECONDARY]);
  });

  it("declares every registered provider as a webview view", () => {
    const contributed = Object.values(manifest.contributes.views).flat();
    for (const id of panes) {
      const view = contributed.find((entry) => entry.id === id);
      assert.ok(view, `${id} is contributed`);
      assert.equal(view.type, "webview", `${id} is a webview view`);
      assert.ok(view.name, `${id} is named`);
    }
  });

  it("activates when either pane is revealed", () => {
    for (const id of panes) {
      assert.ok(manifest.activationEvents.includes(`onView:${id}`), `onView:${id}`);
    }
  });

  it("shows the view-title actions in both panes", () => {
    const title = manifest.contributes.menus["view/title"] ?? [];
    for (const command of ["oxide.newSession", "oxide.resumeSession", "oxide.openTerminal"]) {
      const item = title.find((entry) => entry.command === command);
      assert.ok(item, `${command} has a view/title entry`);
      for (const id of panes) {
        assert.ok(item.when?.includes(id), `${command} is offered in ${id}`);
      }
    }
  });

  it("ships an icon for each container", () => {
    for (const container of [...activitybar, ...secondarySidebar]) {
      assert.ok(fs.existsSync(path.join(root, container.icon)), `${container.icon} exists`);
    }
  });
});
