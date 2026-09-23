# Desktop app

The `oxide-desktop` package (`crates/desktop`) is a Tauri v2 front-end for the
same agent the terminal CLI runs. The goal is one configuration and one session
store shared by both front-ends, with a GUI that can manage **multiple projects**
(cross-repo) and show each project's session list.

## Workspace

The repository is a Cargo workspace:

```
crates/
  core/       shared agent core (config, providers, tools, MCP, sessions,
              snapshots, plugins, agent loop, runner)
  cli/        the `oxide` terminal binary (TUI + print/json/rpc modes)
  desktop/    the `oxide-desktop` app
```

The Cargo package names are `oxide-core`, `oxide` (the CLI), and
`oxide-desktop`. `oxide-core` owns everything the agent needs; the CLI and
desktop are thin front-ends over it, and `oxide-desktop` reuses
`oxide_core::config` and `oxide_core::session` directly, so both read the same
`config.json`, `auth.json`, `settings.json`, and `sessions/` tree.

## Desktop layout

```
crates/desktop/
  src/
    lib.rs          re-exports the GUI-free library
    manager.rs      project registry + session aggregation (shared, tested)
    turn.rs         starts an agent turn against a project (shared, tested)
    approvals.rs    persisted per-project approval rules (shared, tested)
    approval.rs     interactive approve/deny broker (gui feature)
    commands.rs     Tauri commands (gui feature)
    main.rs         Tauri entry point (gui feature)
  ui/               front-end: index.html, app.js, style.css
  capabilities/     Tauri capability (core:default, for events)
  tauri.conf.json   window + bundle config
  entitlements.plist  macOS signing entitlements
  icons/            app icons (PNG, .icns, .ico)
```

The `gui` feature is **off by default** so `cargo build` / `cargo test` /
`cargo clippy` stay free of the Tauri dependency tree. The manager and turn
logic build and test without it.

## Interface

The window follows a Codex-style layout:

- **Sidebar** — the `Oxide` brand, a **New task** button, a project switcher
  (dropdown with add/remove and a searchable project list), the thread list for
  the selected project (or every project via **All projects**), and a footer
  with **Connect**, theme, tool approvals, and help.
- **Top bar** — the current thread title and the active provider.
- **Conversation** — a centered 760px column. User messages are right-aligned
  bubbles; assistant replies render Markdown. Tool calls are compact cards
  showing the call (e.g. `bash cargo test --all`); they expand automatically for
  diffs and errors and can be clicked open/closed. `write`/`edit` results get a
  colored diff.
- **Composer** — a floating rounded box with the model and reasoning chips on
  the left and the send/stop controls on the right; the status and token/cost
  usage sit just below it.

## Running

```sh
cargo run -p oxide-desktop --features gui
```

## Sharing configuration with the CLI

`oxide-desktop` does not have its own settings. It calls
`oxide_core::config::Config::load(project, ...)` for whichever project is
selected, which reads the global `config.json` and `auth.json` and loads that
project's ecosystem — exactly what `oxide` does when launched in that folder.
Credentials added with the CLI's `/login` are therefore available to the
desktop, and vice versa. Project trust (`trust.json`) is resolved the same way
the CLI resolves it before a run.

The **Connect** button stores credentials through `oxide_core::auth`: a new key
via `auth::connect` (which also makes that provider active), or an existing
stored provider via `auth::select_stored`. Model and base URL are persisted with
`Config::persist_selection_at`, the same writer the CLI uses.

## Multiple projects (cross-repo)

`manager::DesktopManager` keeps a project registry at
`<config>/Oxide/desktop/projects.json` (same config directory as the CLI). The
sidebar shows two kinds of project:

- **Added** — folders the user added in the desktop, persisted in the registry.
- **Discovered** — projects seen in the shared session store that were never
  added (e.g. opened only from the terminal). They are derived by grouping
  `SessionLog::list_all()` by the session's `cwd`.

Each row reports its session count and latest activity. Selecting one lists that
project's sessions (`SessionLog::list`) with previews; the **All** toggle in the
sessions header switches to the flat cross-repo list from `all_sessions`, where
opening a session also switches to its project. Sessions can be renamed
(`SessionLog::rename`) and deleted (`SessionLog::delete`) from the list.

## Agent turns

`turn::start_turn(project, prompt, session, approve, reasoning)` loads the
project's config, resolves the session (`new` / `latest` / an id), appends the
user message through `oxide_core::runner::begin_session`, and spawns the shared
agent loop with `runner::spawn_agent`, returning a stream of `AgentEvent`s plus
the run's `Steering` handles and its cooperative `Cancel` flag. The Tauri command serializes events with
`oxide_core::cli::event_json` (the same Pi-shaped JSON the CLI emits in
`--mode json`), attaches any `DiffPreview`, and forwards them over
`agent-event` tagged with a run id.

- **Approvals** — `approval.rs` implements `Approver`. When a rule resolves to
  `ask`, it emits `approval-request` and awaits the UI's `resolve_approval`
  (`deny`, `once`, or `always`); `always` records a per-project rule in
  `ApprovalStore` (`<config>/Oxide/desktop/approvals.json`) so the prompt does
  not repeat for that tool. The 🔒 dialog lists and clears those rules. An
  unanswered request denies after a 5-minute timeout so a turn cannot hang.
- **Cancel / steer** — `send_prompt` returns a run id immediately and runs the
  turn in the background. `cancel_run` sets the run's cooperative `Cancel` flag
  (`oxide_core::agent::Cancel`): the loop finishes the current step — recording
  a result for any planned tool calls so the session stays a valid
  call/result sequence — and ends cleanly, with a 5-second force-abort fallback
  if it is stuck. `steer_run` pushes into the interleaved or follow-up steering
  queue.

## Models and reasoning

The top bar exposes the same controls the CLI has:

- **Model** — opens a picker backed by `list_models`, which uses
  `oxide_core::config::provider_configs` (shared with the CLI's `/models`) to
  fetch every logged-in provider's catalog. Choosing one calls `set_model`,
  which applies it through `apply_provider` / `apply_login_options` and
  persists with `Config::persist_selection_at`.
- **Reasoning** — cycles `auto → off → low → medium → high`.

Reasoning is sent per turn as a `--reasoning`-equivalent override; `start_turn`
passes it to `Config::load`, so it doesn't rewrite the stored config.

## Usage

Opening a session restores its cumulative `usage_totals()` (input/output tokens
and cost); live turns update the footer from each `usage` event, including a
rough context percentage using `config.context_window()`.

## Themes

The desktop ships built-in **Dark** and **Light** themes (default Dark) and
reads the same `.oxide/themes/<name>.json` (project) and
`<config>/Oxide/themes/<name>.json` (global) files the CLI uses. Each theme
resolves every slot to a `#rrggbb` string; the desktop maps them onto CSS
variables. Slots cover the surfaces (`background`, `sidebar`, `panel`,
`panel_2`, `panel_3`, `border`, `text`, `dim`, `faint`) as well as the semantic
colors (`accent`, `user`, `assistant`, `success`, `tool`, `error`, `info`,
`tool_pending_bg`, `thinking_*`). A custom theme file overrides only the slots it
sets, on top of Dark. `set_theme` writes the `theme` key in `config.json`, which
the CLI already reads as its startup default, and the choice is re-applied on
launch.

## Keyboard shortcuts

| Key | Action |
| --- | --- |
| `Enter` | Send; while busy, steer the running turn |
| `Shift+Enter` | Newline |
| `Alt+Enter` | Queue a follow-up |
| `Shift+Tab` / `Ctrl+R` | Cycle reasoning |
| `Ctrl+K` | Model picker |
| `Ctrl+/` | Shortcut help |
| `Escape` | Close any dialog |

## Rendering

Assistant replies render as Markdown: headings, ordered/unordered lists
(including `- [ ]` tasks), blockquotes, rules, pipe tables, fenced code with
lightweight syntax highlighting (Rust, JS/TS, Python, Go, Bash, JSON), and
inline emphasis/code/links. Tool results render as panels; `write`/`edit`
results include a colored diff.

## Packaging

Icons are checked in (`icons/`). Build a bundle with the Tauri CLI:

```sh
npx @tauri-apps/cli@^2 build --features gui          # release
npx @tauri-apps/cli@^2 build --features gui --debug  # faster, unsigned dev bundle
```

`bundle.targets` is `all`, so each platform gets its native formats (`.app` /
`.dmg`, `.msi` / NSIS `.exe`, `.deb` / `.rpm` / AppImage). The macOS build uses
`entitlements.plist` (JIT for the WebView, outbound network). Signing and
notarization are automatic when the usual Developer ID variables are set:

- **macOS**: `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`,
  `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID`.
- **Windows**: the Authenticode certificate variables for `tauri-action`.
- **Linux**: no signing; `.deb`/`.rpm`/AppImage as-is.

Without a Developer ID, `.github/workflows/desktop.yml` — which builds macOS
(arm64 + x64), Linux, and Windows on a tag push and drafts a release — sets
`APPLE_SIGNING_IDENTITY=-`, so Tauri **ad-hoc signs** the macOS bundle. The
signature is valid, but the app is not notarized and macOS quarantines the
download, so the first launch must be approved in **System Settings → Privacy &
Security → Open Anyway**, or the app moved to `/Applications` and the quarantine
cleared with `xattr -dr com.apple.quarantine /Applications/Oxide.app`. An
unsigned bundle is instead rejected outright as *damaged* on Apple Silicon, so
the fallback matters. Auto-update artifacts are not enabled yet (they need a
signing key).

## Signing secrets

`desktop.yml` reads the signing material from repository secrets
(**Settings → Secrets and variables → Actions → New repository secret**).
Nothing is required to build the bundles; without a certificate the macOS app is
only ad-hoc signed (see above).

### Enroll first

A **Developer ID Application** certificate can only be created by an Apple ID
enrolled in the [Apple Developer Program](https://developer.apple.com/programs/enroll/)
($99/year). An unenrolled Apple ID that opens
`developer.apple.com/account/resources` gets *Access Unavailable*: enroll as an
**Individual** (usually minutes to ~48 h; needs your legal name/address and an
identity check) or as an **Organization** (needs a D-U-N-S number and can take
days to weeks). Enable two-factor authentication on the account. Until then
there is nothing to sign with and the ad-hoc fallback is the only option.

### macOS certificate

1. Create a **Developer ID Application** certificate:
   - Xcode — *Settings → Accounts → (team) → Manage Certificates → **+** →
     Developer ID Application*, or
   - [developer.apple.com](https://developer.apple.com/account/resources/certificates)
     → *Certificates → **+** → Developer ID Application* (make the CSR with
     Keychain Access → *Certificate Assistant → Request a Certificate From a
     Certificate Authority*).
2. Export it — *Keychain Access → login → My Certificates*, right-click the
   certificate → *Export*, save as `.p12` with a password.
3. Read the certificate common name on the same Mac:
   `security find-identity -v -p codesigning`.

Then either use your **Apple ID**, or an **App Store Connect API key**, for
notarization — not both.

**Apple ID** (`tauri-action` notarizes and staples automatically):

| Secret | Value |
| --- | --- |
| `APPLE_CERTIFICATE` | base64 of the `.p12` (`base64 -i cert.p12 \| pbcopy`; Linux `base64 -w0 cert.p12`) |
| `APPLE_CERTIFICATE_PASSWORD` | the `.p12` export password |
| `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: Jayson Wu (ABCDE12345)` |
| `APPLE_ID` | the enrolled Apple ID email |
| `APPLE_PASSWORD` | an **app-specific password** (account.apple.com → *Sign-In and Security → App-Specific Passwords*) |
| `APPLE_TEAM_ID` | 10-character Team ID (developer.apple.com → *Membership details*) |

**App Store Connect API key** (create under *Users and Access → Integrations →
App Store Connect API*, role *Admin* or *App Manager*):

| Secret | Value |
| --- | --- |
| `APPLE_API_ISSUER` | the issuer UUID shown above the key list |
| `APPLE_API_KEY` | the key ID (the `AuthKey_<id>.p8` file name) |
| `APPLE_API_KEY_P8` | base64 of the `.p8` (`base64 -i AuthKey_XXXX.p8 \| pbcopy`) |

The workflow decodes `APPLE_API_KEY_P8` to `$RUNNER_TEMP/AuthKey.p8` and sets
`APPLE_API_KEY_PATH`; the `.p8` can only be downloaded once, so store it
somewhere safe.

### Tauri updater keys

Only needed if Tauri's updater is enabled — the app does not ship update
artifacts yet. Generate a key pair and keep the private half backed up (losing
it means existing installs can never accept an update):

```sh
npx @tauri-apps/cli@^2 signer generate -w ~/.tauri/oxide.key
```

Put the key text in `TAURI_SIGNING_PRIVATE_KEY`, its password in
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, and the printed public key in
`tauri.conf.json` as `plugins.updater.pubkey`.

### From the CLI

On the machine that holds the `.p12`:

```sh
base64 -i cert.p12 | gh secret set APPLE_CERTIFICATE
gh secret set APPLE_CERTIFICATE_PASSWORD
gh secret set APPLE_SIGNING_IDENTITY --body 'Developer ID Application: Your Name (ABCDE12345)'
gh secret set APPLE_ID --body 'you@example.com'
gh secret set APPLE_PASSWORD              # paste the app-specific password
gh secret set APPLE_TEAM_ID --body 'ABCDE12345'
```

`gh secret set NAME` without `--body` prompts, so the value never lands in your
shell history.

## Release assets

Each `v*` release carries prebuilt bundles for every platform. Pick the asset
whose platform matches the machine (`<version>` is the release tag without the
leading `v`, e.g. `0.16.1`):

| Asset | Platform |
| --- | --- |
| `Oxide_<version>_aarch64.dmg` | macOS, Apple Silicon (`uname -m` → `arm64`) |
| `Oxide_<version>_x64.dmg` | macOS, Intel (`uname -m` → `x86_64`) |
| `Oxide_<version>_amd64.deb` | Linux x64 (Debian/Ubuntu) |
| `Oxide_<version>_amd64.AppImage` | Linux x64 (portable) |
| `Oxide-<version>-1.x86_64.rpm` | Linux x64 (Fedora/RHEL) |
| `Oxide_<version>_x64-setup.exe` | Windows x64 (NSIS installer) |
| `Oxide_<version>_x64_en-US.msi` | Windows x64 (MSI) |

The same release also holds the CLI archives
(`Oxide-v<version>-<platform>.tar.gz` and `Oxide-v<version>-win32-x64.zip`) plus
`install.sh`/`install.ps1`; see the README's Installation section for the CLI.
