import type { ElectrobunConfig } from "electrobun";

// A release pipeline signs the bundle by naming the identity and the notary
// credentials in the environment (`ELECTROBUN_*`, which is what Hutch reads); a
// build with none of them still produces an installable bundle. Read through
// `globalThis` so the config loads wherever Hutch evaluates it.
const env: Record<string, string | undefined> =
	(globalThis as { process?: { env?: Record<string, string | undefined> } }).process?.env ?? {};
const identity = env.ELECTROBUN_DEVELOPER_ID ?? "";
// Either authentication method is all three of its variables or none of them:
// a partial set (the key id alone, say) would turn notarization on with nothing
// to notarize with, and the release build would fail at the Apple submission
// rather than at the door.
const notary = Boolean(
	(env.ELECTROBUN_APPLEAPIKEY &&
		env.ELECTROBUN_APPLEAPIKEYPATH &&
		env.ELECTROBUN_APPLEAPIISSUER) ||
		(env.ELECTROBUN_APPLEID && env.ELECTROBUN_APPLEIDPASS && env.ELECTROBUN_TEAMID),
);

// The window, the session store and the agent runs are the Rust main process
// (src/main.rs); `ui/` is what that window loads. The version here is the one a
// release reports and `scripts/set-version.sh` writes into, beside the workspace
// `Cargo.toml`.
//
// The front-end is copied verbatim rather than declared as a view: it is one
// plain script with no imports to resolve, and it talks to the host over the
// preload's own bridge, so there is nothing for a bundler to do. A view's files
// are served from `views://<name>/`, which is the copy destination's `views/`
// folder.
export default {
	app: {
		name: "Oxide",
		identifier: "dev.oxide.desktop",
		version: "0.0.0",
		description: "Desktop app for Oxide: manage multiple projects and sessions",
	},
	build: {
		mainProcess: "rust",
		rust: {
			manifest: "Cargo.toml",
			binary: "oxide-desktop",
		},
		copy: {
			"ui/index.html": "views/main/index.html",
			"ui/app.js": "views/main/app.js",
			"ui/style.css": "views/main/style.css",
		},
		mac: {
			icons: "icons/icon.iconset",
			// A build with no signing identity in the environment is not signed; a
			// release is, and is notarized when the notary credentials are there
			// too. Both are the pipeline's to provide, never this file's.
			codesign: identity !== "",
			notarize: notary,
			entitlements: {
				// The page is a WKWebView running the app's own script, and the app
				// reaches model and MCP endpoints from the main process.
				"com.apple.security.cs.allow-jit": true,
				"com.apple.security.cs.allow-unsigned-executable-memory": true,
				"com.apple.security.network.client": true,
			},
		},
		win: {
			icon: "icons/icon.ico",
		},
		linux: {
			icon: "icons/icon.png",
		},
	},
	runtime: {
		// The window is the app; closing it ends the process rather than leaving a
		// resident app with a turn streaming into no window.
		exitOnLastWindowClosed: true,
	},
} satisfies ElectrobunConfig;
