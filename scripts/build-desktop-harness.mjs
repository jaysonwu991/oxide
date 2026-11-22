import { copyFileSync, mkdirSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const desktopRoot = fileURLToPath(new URL("../crates/desktop/", import.meta.url));
const hostArch = process.arch === "arm64" ? "arm64" : process.arch === "x64" ? "x64" : process.arch;
const requestedArch = process.env.OXIDE_DESKTOP_ARCH || hostArch;
const targets = {
  darwin: {
    arm64: "aarch64-apple-darwin",
    x64: "x86_64-apple-darwin",
  },
};

const target = requestedArch === hostArch ? "" : targets[process.platform]?.[requestedArch] || "";
if (requestedArch !== hostArch && !target) {
  throw new Error(`cannot build the ${requestedArch} desktop harness on ${process.platform}/${hostArch}`);
}

const args = ["build", "--release"];
if (target) {
  args.push("--target", target);
}
execFileSync("cargo", args, { cwd: desktopRoot, stdio: "inherit" });

if (target) {
  const binary = process.platform === "win32" ? "oxide-desktop.exe" : "oxide-desktop";
  const source = `${desktopRoot}target/${target}/release/${binary}`;
  const destination = `${desktopRoot}target/release/${binary}`;
  mkdirSync(`${desktopRoot}target/release`, { recursive: true });
  copyFileSync(source, destination);
}
