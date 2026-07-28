import { chmodSync, rmSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const packageDir = path.resolve(scriptDir, "..");
const compiler = path.join(packageDir, "node_modules", "typescript", "bin", "tsc");

rmSync(path.join(packageDir, "dist"), { force: true, recursive: true });

const result = spawnSync(process.execPath, [compiler, "-p", "tsconfig.json"], {
  cwd: packageDir,
  stdio: "inherit"
});

if (result.status !== 0) {
  process.exit(result.status ?? 1);
}

chmodSync(path.join(packageDir, "dist", "cli.js"), 0o755);
