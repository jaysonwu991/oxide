// Checks the Tauri host against the desktop page without opening a window. Rust
// tests cover the host's parsers; this joins the files that have to agree on a
// packet, a native service or a release train.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url));
const root = fileURLToPath(new URL("../../", import.meta.url));
const read = (path) => readFileSync(path, "utf8").replace(/\r\n/g, "\n");

const failures = [];
const check = (name, condition, detail = "") => {
  if (condition) {
    console.log(`  ok   ${name}`);
    return;
  }
  console.log(`  FAIL ${name}${detail ? ` — ${detail}` : ""}`);
  failures.push(name);
};

const manifest = read(`${here}Cargo.toml`);
const config = JSON.parse(read(`${here}tauri.conf.json`));
const main = read(`${here}src/main.rs`);
const menu = read(`${here}src/menu.rs`);
const page = read(`${root}crates/desktop/ui/app.js`);
const html = read(`${root}crates/desktop/ui/index.html`);
const sheet = read(`${root}crates/desktop/ui/style.css`);
const updates = read(`${root}crates/core/src/updates.rs`);
const workflow = read(`${root}.github/workflows/desktop.yml`);
const releaseDrafter = read(`${root}.github/release-drafter.desktop.yml`);
const cliReleaseDrafter = read(`${root}.github/release-drafter.cli.yml`);
const vscodeReleaseDrafter = read(`${root}.github/release-drafter.vscode.yml`);
const packageScript = read(`${here}package.sh`);

console.log("the pinned Tauri host");
check("pinned the latest Tauri 2 runtime", /^tauri = "=2\.12\.7"$/m.test(manifest));
check(
  "pinned matching native plugins",
  /^tauri-plugin-dialog = "=2\.8\.4"$/m.test(manifest) &&
    /^tauri-plugin-notification = "=2\.8\.1"$/m.test(manifest) &&
    /^tauri-plugin-opener = "=2\.7\.3"$/m.test(manifest),
);
check(
  "hosted the desktop page",
  config.build.frontendDist === "ui" && config.app.security.csp === null,
  config.build.frontendDist,
);
check(
  "kept one Oxide window with its minimum size",
  config.productName === "Oxide" &&
    config.identifier === "dev.oxide.desktop" &&
    config.app.windows.length === 1 &&
    config.app.windows[0].label === "main" &&
    config.app.windows[0].minWidth === 820 &&
    config.app.windows[0].minHeight === 560,
);
check(
  "created the maximized window before revealing the page",
  config.app.windows[0].visible === false &&
    config.app.windows[0].maximized === true &&
    /fn native_ready\(app: &AppHandle/.test(main) &&
    /window\.show\(\)/.test(main) &&
    !/window\.maximize\(\)/.test(main),
);
check(
  "kept the app's topbar draggable",
  /<header class="topbar" data-tauri-drag-region>/.test(html) &&
    /\.topbar \{[^}]*-webkit-app-region:\s*drag/.test(sheet),
);

console.log("the packet bridge");
check(
  "installed the Tauri bridge in the shared page",
  /const nativeTauri = window\.__TAURI__/.test(page) &&
    /nativeTauri\.core\.invoke\("oxide_message", \{ message: raw \}\)/.test(page) &&
    /nativeTauri\.event\.listen\("oxide:to-page"/.test(page),
);
check(
  "held launch packets until the page subscribed",
  /window\.__electrobunPendingHostMessages\.push\(raw\)/.test(page) &&
    /command: "native_ready"/.test(page) &&
    /pending: Vec<String>/.test(main) &&
    /fn waits_for_page/.test(main) &&
    /inner\.ready \|\| !waits_for_page/.test(main) &&
    /state\.delivery\.flush/.test(main),
);
check(
  "answered every shared-page request in the page's envelope",
  /"type": "response"/.test(main) &&
    /"id": id/.test(main) &&
    /"success": true/.test(main) &&
    /"success": false/.test(main),
);

console.log("the host-owned services");
const hostCalls = [
  "set_native_language",
  "set_native_shortcuts",
  "pick_folder",
  "open_url",
  "set_zoom",
  "notify",
  "set_badge",
  "set_awake",
];
for (const name of hostCalls) {
  check(
    `handled ${name} in the Tauri host`,
    new RegExp(`"${name}"`).test(main),
  );
}
check(
  "limited external links to HTTP(S)",
  /fn is_openable_url/.test(main) &&
    /url\.starts_with\("http:\/\/"\)/.test(main) &&
    /url\.starts_with\("https:\/\/"\)/.test(main),
);
check(
  "rebuilt translated native menus",
  /pub fn install\(app: &AppHandle/.test(menu) &&
    /state\.text\("ui\.newTab"/.test(menu) &&
    /state\.accelerator\("settings"/.test(menu) &&
    !/state\.text\("menu\.pulls"/.test(menu) &&
    !/"open-pulls" =>/.test(menu),
);
check(
  "relaunched in front only after the engine accepts the restart",
  /packet\["id"\] == "restart"/.test(main) &&
    /restart_app\(&app\)/.test(main) &&
    /while kill -0 "\$1"/.test(main) &&
    /\/usr\/bin\/open \\"\$2\\"/.test(main) &&
    !/\/usr\/bin\/open -g/.test(main),
);

console.log("the release contract");
check(
  "gives desktop update requests this build's version",
  /"check_updates" \| "install_update" \| "download_update" => \{[\s\S]*?args\["current"\] = json!\(current\)/.test(
    main,
  ) && /Component::Desktop/.test(updates),
);
check(
  "published the desktop release train and stable alias",
  /desktop-v\*/.test(workflow) &&
    /desktop-latest/.test(workflow) &&
    /tag-template: 'desktop-v\$RESOLVED_VERSION'/.test(releaseDrafter),
);
check(
  "kept all three release notes free of duplicate changelog and contributor sections",
  [cliReleaseDrafter, releaseDrafter, vscodeReleaseDrafter].every(
    (notes) =>
      !/## Full Changelog|## New Contributors|## Contributors|NEW_CONTRIBUTORS|CONTRIBUTORS/.test(
        notes,
      ),
  ),
);
check(
  "kept each release draft out of the other components' logs",
  [
    [cliReleaseDrafter, ["desktop", "vscode"]],
    [releaseDrafter, ["cli", "vscode"]],
    [vscodeReleaseDrafter, ["cli", "desktop"]],
  ].every(([notes, excluded]) => {
    const section = notes.match(/(?:^|\n)exclude-labels:\n((?:  - '[^']+'\n)+)/);
    return excluded.every((label) => section?.[1].includes(`  - '${label}'`));
  }),
);
check(
  "kept the CLI release notes on the install scripts",
  /raw\.githubusercontent\.com\/\$OWNER\/\$REPOSITORY\/main\/install\.sh/.test(cliReleaseDrafter) &&
    /raw\.githubusercontent\.com\/\$OWNER\/\$REPOSITORY\/main\/install\.ps1/.test(
      cliReleaseDrafter,
    ),
);
check(
  "kept the VS Code release notes on the VSIX",
  /Download `oxide-vscode\.vsix`/.test(vscodeReleaseDrafter) &&
    !/Setup\.tar\.gz/.test(vscodeReleaseDrafter),
);
check(
  "packaged the updater asset names",
  /Oxide-macos-\$arch\.dmg/.test(packageScript) &&
    /Oxide-linux-\$arch-Setup\.tar\.gz/.test(packageScript) &&
    /Oxide-win-\$arch-Setup\.exe/.test(packageScript),
);
check(
  "built the Intel bundle from the x86_64 target on Apple Silicon",
  /matrix\.platform == 'macos-latest' && matrix\.arch == 'x64' && 'x86_64-apple-darwin'/.test(
    workflow,
  ) &&
    /rust_target=x86_64-apple-darwin/.test(packageScript) &&
    /cargo build --release --target "\$rust_target"/.test(packageScript) &&
    /binary="target\/\$rust_target\/release\/oxide-desktop"/.test(packageScript) &&
    /binary=\.\/artifacts\/Oxide\.app\/Contents\/MacOS\/oxide-desktop/.test(workflow),
);

if (failures.length) {
  console.error(`\n${failures.length} Tauri shell check(s) failed`);
  process.exit(1);
}
console.log("\nall Tauri shell checks passed");
