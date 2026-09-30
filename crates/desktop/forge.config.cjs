const path = require("node:path");

const windows = process.platform === "win32";
const hostName = windows ? "oxide-desktop-host.exe" : "oxide-desktop-host";
const host = process.env.OXIDE_DESKTOP_HOST
  ? path.resolve(process.env.OXIDE_DESKTOP_HOST)
  : path.resolve(__dirname, "..", "..", "target", "release", hostName);
const signingIdentity = process.env.APPLE_SIGNING_IDENTITY || "-";

const notarize = (() => {
  if (process.env.APPLE_API_KEY_PATH && process.env.APPLE_API_KEY && process.env.APPLE_API_ISSUER) {
    return {
      appleApiKey: process.env.APPLE_API_KEY_PATH,
      appleApiKeyId: process.env.APPLE_API_KEY,
      appleApiIssuer: process.env.APPLE_API_ISSUER,
    };
  }
  if (process.env.APPLE_ID && process.env.APPLE_PASSWORD && process.env.APPLE_TEAM_ID) {
    return {
      appleId: process.env.APPLE_ID,
      appleIdPassword: process.env.APPLE_PASSWORD,
      teamId: process.env.APPLE_TEAM_ID,
    };
  }
  return undefined;
})();

module.exports = {
  packagerConfig: {
    name: "Oxide",
    executableName: "Oxide",
    appBundleId: "dev.oxide.desktop",
    appCategoryType: "public.app-category.developer-tools",
    asar: true,
    icon: path.join(
      __dirname,
      "icons",
      process.platform === "win32" ? "icon.ico" : process.platform === "darwin" ? "icon.icns" : "icon.png",
    ),
    extraResource: [host],
    osxSign:
      process.platform === "darwin"
        ? {
            identity: signingIdentity,
            identityValidation: signingIdentity !== "-",
            continueOnError: false,
            // Hardened library validation requires a shared Developer ID team.
            // Ad-hoc development builds have no team, so leave it off there.
            optionsForFile: () => ({ hardenedRuntime: signingIdentity !== "-" }),
          }
        : undefined,
    osxNotarize: notarize,
  },
  makers: [
    { name: "@electron-forge/maker-dmg", platforms: ["darwin"], config: { icon: path.join(__dirname, "icons", "icon.icns") } },
    { name: "@electron-forge/maker-zip", platforms: ["darwin", "win32"] },
    { name: "@electron-forge/maker-squirrel", platforms: ["win32"], config: { name: "oxide_desktop", setupIcon: path.join(__dirname, "icons", "icon.ico") } },
    { name: "@electron-forge/maker-deb", platforms: ["linux"], config: { options: { maintainer: "Oxide", homepage: "https://github.com/jaysonwu991/oxide", icon: path.join(__dirname, "icons", "icon.png") } } },
    { name: "@electron-forge/maker-rpm", platforms: ["linux"], config: { options: { homepage: "https://github.com/jaysonwu991/oxide", icon: path.join(__dirname, "icons", "icon.png") } } },
  ],
};
