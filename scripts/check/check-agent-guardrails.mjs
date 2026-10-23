import { spawn, spawnSync } from "node:child_process";
import { lstatSync, readFileSync } from "node:fs";
import { createInterface } from "node:readline";

process.chdir(git(["rev-parse", "--show-toplevel"]).trim());

const paths = ["crates", "src-tauri"];
const rules = [
  { name: "panic", pattern: /\bpanic!\s*\(/i },
  { name: "todo/unimplemented", pattern: /\b(todo|unimplemented)!\s*\(/i },
  { name: "debug/print", pattern: /\b(dbg|print|eprint|println|eprintln)!\s*\(/i },
  { name: "mem::forget", pattern: /\bmem::forget\s*\(/i },
];
const testPath =
  /(^|\/|\\)tests?(\/|\\)|(^|\/|\\)[^/\\]+_tests?\.rs$|(^|\/|\\)tests\.rs$/i;

function git(args) {
  const result = spawnSync("git", args, { encoding: "utf8" });
  if (result.error) {
    throw new Error(`git ${args.join(" ")} failed: ${result.error.message}`);
  }
  if (result.status !== 0) {
    const detail = `${result.stderr ?? ""}${result.stdout ?? ""}`.trim();
    throw new Error(`git ${args.join(" ")} failed${detail ? `: ${detail}` : ""}`);
  }
  return result.stdout ?? "";
}

function hasCommit(ref) {
  const result = spawnSync("git", ["rev-parse", "--verify", "--quiet", `${ref}^{commit}`], {
    encoding: "utf8",
  });
  return result.status === 0;
}

const requestedBase = process.argv.slice(2).find((arg) => arg && !arg.startsWith("-"));
const diffs = [];

if (requestedBase) {
  if (!hasCommit(requestedBase)) {
    throw new Error(`Guardrail check requires a valid base ref, got '${requestedBase}'.`);
  }
  diffs.push(["diff", "--unified=0", `${requestedBase}...HEAD`, "--", ...paths]);
} else if (process.env.GITHUB_BASE_REF) {
  git(["fetch", "--no-tags", "--quiet", "origin", process.env.GITHUB_BASE_REF]);
  diffs.push(["diff", "--unified=0", `origin/${process.env.GITHUB_BASE_REF}...HEAD`, "--", ...paths]);
} else if (hasCommit("HEAD^")) {
  diffs.push(["diff", "--unified=0", "HEAD^", "HEAD", "--", ...paths]);
}

diffs.push(["diff", "--unified=0", "--", ...paths]);
diffs.push(["diff", "--cached", "--unified=0", "--", ...paths]);

const violations = [];

function checkAddedLine(path, line) {
  if (testPath.test(path) || (line.includes("fixture") && /\.(unwrap|expect)\s*\(/i.test(line))) {
    return;
  }
  for (const rule of rules) {
    if (rule.pattern.test(line) && violations.length < 40) {
      violations.push(`[${rule.name}] ${path}: ${line}`);
    }
  }
}

for (const args of diffs) {
  // Release branches can have a large diff. Read every line without buffering
  // the whole patch in spawnSync or truncating the end of the check.
  const child = spawn("git", args, { stdio: ["ignore", "pipe", "pipe"], windowsHide: true });
  let stderr = "";
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (chunk) => { stderr = (stderr + chunk).slice(-8_192); });
  const finished = new Promise((resolve) => {
    child.once("error", (error) => resolve({ error }));
    child.once("close", (code) => resolve({ code }));
  });
  const lines = createInterface({ input: child.stdout, crlfDelay: Infinity });
  let currentPath = "";
  try {
    lines.on("line", (line) => {
      const header = /^diff --git a\/(.+) b\/(.+)$/.exec(line);
      if (header) {
        currentPath = header[2];
        return;
      }
      if (!line.startsWith("+") || line.startsWith("+++")) {
        return;
      }
      checkAddedLine(currentPath, line);
    });
    const result = await finished;
    if (result.error || result.code !== 0) {
      const detail = result.error?.message ?? stderr.trim();
      throw new Error(`git ${args.join(" ")} failed${detail ? `: ${detail}` : ""}`);
    }
  } catch (error) {
    child.kill();
    throw error;
  } finally {
    lines.close();
  }
}

const untracked = git(["ls-files", "--others", "--exclude-standard", "-z", "--", ...paths]);
for (const path of untracked.split("\0")) {
  if (!path.endsWith(".rs") || testPath.test(path) || !lstatSync(path).isFile()) {
    continue;
  }
  for (const line of readFileSync(path, "utf8").split(/\r?\n/)) {
    checkAddedLine(path, `+${line}`);
  }
}

if (violations.length > 0) {
  console.error(violations.slice(0, 40).join("\n"));
  console.error("New production guardrail violations detected. Use typed error paths instead.");
  process.exit(1);
}
