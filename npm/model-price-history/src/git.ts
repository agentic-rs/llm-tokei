import { spawnSync } from "node:child_process";

export type GitCommit = {
  commit_sha: string;
  parent_commit_sha: string | undefined;
  ts: string;
};

export type ChangedPath = {
  op: "delete" | "write";
  path: string;
};

export type GitTreeEntry = {
  mode: string;
  path: string;
};

const MAX_BUFFER = 256 * 1024 * 1024;

export function assertCompleteRepository(repositoryPath: string): void {
  const shallow = runGitText(repositoryPath, ["rev-parse", "--is-shallow-repository"]).trim();
  if (shallow === "true") {
    throw new Error(
      `models.dev repository ${JSON.stringify(repositoryPath)} is shallow; fetch the complete history before generating price history`
    );
  }
}

export function resolveCommit(repositoryPath: string, ref: string): string {
  return runGitText(repositoryPath, ["rev-parse", "--verify", `${ref}^{commit}`]).trim();
}

export function listFirstParentCommits(repositoryPath: string, commitSha: string): GitCommit[] {
  const output = runGitText(repositoryPath, [
    "log",
    "--first-parent",
    "--reverse",
    "--format=%H%x00%P%x00%ct",
    commitSha
  ]).trimEnd();

  if (!output) return [];

  return output.split("\n").map((line) => {
    const [commitSha, parents, timestamp] = line.split("\0");
    if (!commitSha || parents === undefined || !timestamp) {
      throw new Error(`could not parse Git history record ${JSON.stringify(line)}`);
    }
    const seconds = Number(timestamp);
    if (!Number.isSafeInteger(seconds)) {
      throw new Error(`commit ${commitSha} has an invalid committer timestamp ${JSON.stringify(timestamp)}`);
    }
    const date = new Date(seconds * 1000);
    if (Number.isNaN(date.getTime())) {
      throw new Error(`commit ${commitSha} has an invalid committer timestamp ${JSON.stringify(timestamp)}`);
    }
    return {
      commit_sha: commitSha,
      parent_commit_sha: parents.split(" ")[0] || undefined,
      ts: date.toISOString()
    };
  });
}

export function listTreeEntries(
  repositoryPath: string,
  commitSha: string,
  paths: readonly string[] = []
): GitTreeEntry[] {
  const args = ["ls-tree", "-r", "-z", commitSha];
  if (paths.length > 0) args.push("--", ...paths);
  return splitNul(runGit(repositoryPath, args)).map((entry) => {
    const tab = entry.indexOf("\t");
    if (tab === -1) throw new Error(`could not parse tree entry ${JSON.stringify(entry)}`);
    const [mode] = entry.slice(0, tab).split(" ");
    const path = entry.slice(tab + 1);
    if (!mode || !path) throw new Error(`could not parse tree entry ${JSON.stringify(entry)}`);
    return { mode, path };
  });
}

export function listChangedPaths(
  repositoryPath: string,
  parentCommitSha: string,
  commitSha: string
): ChangedPath[] {
  const output = runGit(repositoryPath, [
    "diff-tree",
    "--no-commit-id",
    "--no-renames",
    "--name-status",
    "-r",
    "-z",
    parentCommitSha,
    commitSha
  ]);
  const fields = splitNul(output);
  const changes: ChangedPath[] = [];

  for (let index = 0; index < fields.length; ) {
    const status = fields[index++];
    const path = fields[index++];
    if (!status || path === undefined) {
      throw new Error(`could not parse changed paths for commit ${commitSha}`);
    }
    changes.push({ op: status.startsWith("D") ? "delete" : "write", path });
  }

  return changes;
}

export function readFilesAtCommit(
  repositoryPath: string,
  commitSha: string,
  paths: readonly string[]
): Map<string, string> {
  if (paths.length === 0) return new Map();

  const input = Buffer.from(paths.map((path) => `${commitSha}:${path}\n`).join(""));
  const output = runGit(repositoryPath, ["cat-file", "--batch"], input);
  const files = new Map<string, string>();
  let offset = 0;

  for (const path of paths) {
    const header = readLine(output, offset);
    offset = header.next;
    const match = header.value.match(/^[0-9a-f]+ blob (\d+)$/);
    if (!match) {
      throw new Error(`could not read ${path} from commit ${commitSha}: ${header.value}`);
    }
    const length = Number(match[1]);
    const end = offset + length;
    if (!Number.isSafeInteger(length) || end > output.length) {
      throw new Error(`could not read ${path} from commit ${commitSha}: invalid blob length`);
    }
    files.set(path, output.subarray(offset, end).toString("utf8"));
    offset = end;
    if (output[offset] !== 0x0a) {
      throw new Error(`could not read ${path} from commit ${commitSha}: missing blob terminator`);
    }
    offset += 1;
  }

  if (offset !== output.length) {
    throw new Error(`could not read files from commit ${commitSha}: unexpected trailing Git output`);
  }

  return files;
}

function runGit(repositoryPath: string, args: string[], input?: Buffer): Buffer {
  const result = spawnSync("git", ["-C", repositoryPath, ...args], {
    encoding: "buffer",
    input,
    maxBuffer: MAX_BUFFER
  });
  if (result.error) {
    throw new Error(`could not run git: ${result.error.message}`);
  }
  if (result.status !== 0) {
    const stderr = text(result.stderr).trim();
    throw new Error(`git ${args.join(" ")} failed${stderr ? `: ${stderr}` : ""}`);
  }
  return Buffer.from(result.stdout ?? "");
}

function runGitText(repositoryPath: string, args: string[]): string {
  return runGit(repositoryPath, args).toString("utf8");
}

function splitNul(value: Buffer): string[] {
  const fields = value.toString("utf8").split("\0");
  if (fields.at(-1) === "") fields.pop();
  return fields;
}

function readLine(value: Buffer, offset: number): { value: string; next: number } {
  const end = value.indexOf(0x0a, offset);
  if (end === -1) {
    throw new Error("could not parse Git output: missing newline");
  }
  return { value: value.subarray(offset, end).toString("utf8"), next: end + 1 };
}

function text(value: Buffer | string | null): string {
  if (!value) return "";
  return typeof value === "string" ? value : value.toString("utf8");
}
