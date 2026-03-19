#!/usr/bin/env node
"use strict";

const { spawnSync } = require("child_process");
const path = require("path");
const os = require("os");

const PLATFORM_PACKAGES = {
  "darwin-arm64": "@agentusage/cli-darwin-arm64",
  "darwin-x64":   "@agentusage/cli-darwin-x64",
  "linux-x64":    "@agentusage/cli-linux-x64",
  "win32-x64":    "@agentusage/cli-win32-x64",
};

const platformKey = `${os.platform()}-${os.arch()}`;
const pkg = PLATFORM_PACKAGES[platformKey];

if (!pkg) {
  process.stderr.write(
    `agentusage: unsupported platform '${platformKey}'\n` +
    `Supported platforms: ${Object.keys(PLATFORM_PACKAGES).join(", ")}\n` +
    `You can also install from source: https://github.com/agentusage-team/agentusage-cli\n`
  );
  process.exit(1);
}

let pkgDir;
try {
  pkgDir = path.dirname(require.resolve(`${pkg}/package.json`));
} catch {
  process.stderr.write(
    `agentusage: platform package '${pkg}' is not installed.\n` +
    `This is unexpected — please file a bug at https://github.com/agentusage-team/agentusage-cli/issues\n`
  );
  process.exit(1);
}

const ext = os.platform() === "win32" ? ".exe" : "";
const binary = path.join(pkgDir, "bin", `au${ext}`);

const result = spawnSync(binary, process.argv.slice(2), { stdio: "inherit" });

if (result.error) {
  process.stderr.write(`agentusage: failed to run binary: ${result.error.message}\n`);
  process.exit(1);
}

process.exit(result.status ?? 1);
