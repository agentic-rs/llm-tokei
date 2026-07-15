import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

export function createFixtureRepository() {
  const repositoryPath = mkdtempSync(path.join(tmpdir(), "model-price-history-"));
  git(repositoryPath, ["init", "--initial-branch=main"]);
  git(repositoryPath, ["config", "user.email", "test@example.com"]);
  git(repositoryPath, ["config", "user.name", "Model Price History Test"]);

  return {
    commit(message, timestamp) {
      git(repositoryPath, ["add", "--all"]);
      git(repositoryPath, ["commit", "--quiet", "-m", message], {
        GIT_AUTHOR_DATE: timestamp,
        GIT_COMMITTER_DATE: timestamp
      });
      return git(repositoryPath, ["rev-parse", "HEAD"]).trim();
    },
    cleanup() {
      rmSync(repositoryPath, { force: true, recursive: true });
    },
    remove(relativePath) {
      rmSync(path.join(repositoryPath, relativePath));
    },
    repository_path: repositoryPath,
    symlink(relativePath, target) {
      const link = path.join(repositoryPath, relativePath);
      mkdirSync(path.dirname(link), { recursive: true });
      symlinkSync(target, link);
    },
    write(relativePath, source) {
      const target = path.join(repositoryPath, relativePath);
      mkdirSync(path.dirname(target), { recursive: true });
      writeFileSync(target, source, "utf8");
    }
  };
}

function git(repositoryPath, args, environment = {}) {
  return execFileSync("git", ["-C", repositoryPath, ...args], {
    encoding: "utf8",
    env: { ...process.env, ...environment }
  });
}
