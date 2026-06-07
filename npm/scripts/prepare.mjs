#!/usr/bin/env node
import { chmodSync, copyFileSync, cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const platforms = [
  {
    cpu: "arm64",
    description: "macOS arm64 binary for llm-tokei",
    executable: "llm-tokei",
    os: "darwin",
    package: "llm-tokei-darwin-arm64"
  },
  {
    cpu: "x64",
    description: "Linux x64 binary for llm-tokei",
    executable: "llm-tokei",
    os: "linux",
    package: "llm-tokei-linux-x64"
  },
  {
    cpu: "x64",
    description: "Windows x64 binary for llm-tokei",
    executable: "llm-tokei.exe",
    os: "win32",
    package: "llm-tokei-win32-x64"
  }
];

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, "..", "..");

function usage() {
  return `Usage:
  node npm/scripts/prepare.mjs --version <version> [options]

Options:
  --version <version>      Package version to write, e.g. 0.1.8 or 0.1.8-rc.1.
  --scope <scope>          npm scope to use. Default: @tokn-ai.
  --binary-root <path>     Directory containing platform binary package folders. Default: ./npm.
  --out-dir <path>         Generated package output directory. Default: ./dist/npm.
  --help                   Show this help.

Expected binaries:
  <binary-root>/llm-tokei-darwin-arm64/bin/llm-tokei
  <binary-root>/llm-tokei-linux-x64/bin/llm-tokei
  <binary-root>/llm-tokei-win32-x64/bin/llm-tokei.exe
`;
}

function parseArgs(args) {
  const parsed = {
    binaryRoot: path.join(repoRoot, "npm"),
    outDir: path.join(repoRoot, "dist", "npm"),
    scope: "@tokn-ai"
  };

  for (let i = 0; i < args.length; i += 1) {
    const arg = args[i];
    const next = args[i + 1];
    if (arg === "--help" || arg === "-h") {
      parsed.help = true;
    } else if (arg === "--binary-root" && next) {
      parsed.binaryRoot = path.resolve(next);
      i += 1;
    } else if (arg === "--out-dir" && next) {
      parsed.outDir = path.resolve(next);
      i += 1;
    } else if (arg === "--scope" && next) {
      parsed.scope = next;
      i += 1;
    } else if (arg === "--version" && next) {
      parsed.version = next;
      i += 1;
    } else {
      throw new Error(`unknown or incomplete argument: ${arg}`);
    }
  }

  if (parsed.help) {
    return parsed;
  }
  if (!parsed.version) {
    throw new Error("missing required --version");
  }
  if (!parsed.scope.startsWith("@")) {
    throw new Error(`scope must start with @: ${parsed.scope}`);
  }

  return parsed;
}

function renderTemplate(templateName, values) {
  let rendered = readFileSync(path.join(repoRoot, "npm", "templates", templateName), "utf8");
  for (const [key, value] of Object.entries(values)) {
    rendered = rendered.replaceAll(`{{${key}}}`, value);
  }
  return rendered;
}

function npmReadme() {
  return readFileSync(path.join(repoRoot, "README.md"), "utf8")
    .replaceAll("](LICENSE)", "](https://github.com/agentic-rs/llm-tokei/blob/main/LICENSE)")
    .replaceAll("](Cargo.toml)", "](https://github.com/agentic-rs/llm-tokei/blob/main/Cargo.toml)")
    .replaceAll("](docs/usage.md)", "](https://github.com/agentic-rs/llm-tokei/blob/main/docs/usage.md)")
    .replaceAll("![llm-tokei terminal table output](docs/assets/showcase.svg)", "![llm-tokei terminal table output](https://raw.githubusercontent.com/agentic-rs/llm-tokei/main/docs/assets/showcase.svg)");
}

function prepareRootPackage(options) {
  const packageDir = path.join(options.outDir, "llm-tokei");
  mkdirSync(path.join(packageDir, "bin"), { recursive: true });
  writeFileSync(
    path.join(packageDir, "package.json"),
    renderTemplate("root-package.json", {
      scope: options.scope,
      version: options.version
    })
  );
  writeFileSync(path.join(packageDir, "README.md"), npmReadme());
  copyFileSync(path.join(repoRoot, "npm", "bin", "llm-tokei.mjs"), path.join(packageDir, "bin", "llm-tokei.mjs"));
  chmodSync(path.join(packageDir, "bin", "llm-tokei.mjs"), 0o755);
}

function preparePlatformPackage(options, platform) {
  const binaryPath = path.join(options.binaryRoot, platform.package, "bin", platform.executable);
  const packageDir = path.join(options.outDir, platform.package);
  const binDir = path.join(packageDir, "bin");
  mkdirSync(binDir, { recursive: true });
  writeFileSync(
    path.join(packageDir, "package.json"),
    renderTemplate("platform-package.json", {
      cpu: platform.cpu,
      description: platform.description,
      os: platform.os,
      package: platform.package,
      scope: options.scope,
      version: options.version
    })
  );
  cpSync(binaryPath, path.join(binDir, platform.executable));
  if (platform.os !== "win32") {
    chmodSync(path.join(binDir, platform.executable), 0o755);
  }
}

function validateBinaries(options) {
  for (const platform of platforms) {
    const binaryPath = path.join(options.binaryRoot, platform.package, "bin", platform.executable);
    if (!existsSync(binaryPath)) {
      throw new Error(`missing binary for ${platform.package}: ${binaryPath}`);
    }
  }
}

function main() {
  const options = parseArgs(process.argv.slice(2));
  if (options.help) {
    console.log(usage());
    return;
  }
  validateBinaries(options);
  rmSync(options.outDir, { recursive: true, force: true });
  mkdirSync(options.outDir, { recursive: true });
  prepareRootPackage(options);
  for (const platform of platforms) {
    preparePlatformPackage(options, platform);
  }
  console.log(options.outDir);
}

try {
  main();
} catch (error) {
  console.error(`npm prepare: ${error.message}`);
  process.exit(1);
}
