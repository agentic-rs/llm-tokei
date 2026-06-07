#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);

const packageByPlatform = {
  "darwin-arm64": "@tokn/llm-tokei-darwin-arm64",
  "linux-x64": "@tokn/llm-tokei-linux-x64",
  "win32-x64": "@tokn/llm-tokei-win32-x64"
};

const platformKey = `${process.platform}-${process.arch}`;
const packageName = packageByPlatform[platformKey];

if (!packageName) {
  console.error(
    `llm-tokei: unsupported platform ${platformKey}. ` +
      "Supported npm binaries: darwin-arm64, linux-x64, win32-x64."
  );
  process.exit(1);
}

const executable = process.platform === "win32" ? "llm-tokei.exe" : "llm-tokei";
let binaryPath;

try {
  binaryPath = require.resolve(`${packageName}/bin/${executable}`);
} catch (error) {
  console.error(
    `llm-tokei: failed to find ${packageName}. ` +
      "Try reinstalling with npm i -g @tokn/llm-tokei."
  );
  if (process.env.LLM_TOKEI_DEBUG_INSTALL) {
    console.error(error);
  }
  process.exit(1);
}

const result = spawnSync(binaryPath, process.argv.slice(2), { stdio: "inherit" });

if (result.error) {
  console.error(`llm-tokei: failed to run ${binaryPath}: ${result.error.message}`);
  process.exit(1);
}

process.exit(result.status ?? 1);
