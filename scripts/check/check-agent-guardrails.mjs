import { spawnSync } from "node:child_process";

const paths = ["crates", "relay-server", "src-tauri"];
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
const diffParts = [];

if (requestedBase) {
  if (!hasCommit(requestedBase)) {
    throw new Error(`Guardrail check requires a valid base ref, got '${requestedBase}'.`);
  }
  diffParts.push(git(["diff", "--unified=0", `${requestedBase}...HEAD`, "--", ...paths]));
} else if (process.env.GITHUB_BASE_REF) {
  git(["fetch", "--no-tags", "--quiet", "origin", process.env.GITHUB_BASE_REF]);
  diffParts.push(
    git(["diff", "--unified=0", `origin/${process.env.GITHUB_BASE_REF}...HEAD`, "--", ...paths]),
  );
} else if (hasCommit("HEAD^")) {
  diffParts.push(git(["diff", "--unified=0", "HEAD^", "HEAD", "--", ...paths]));
}

diffParts.push(git(["diff", "--unified=0", "--", ...paths]));
diffParts.push(git(["diff", "--cached", "--unified=0", "--", ...paths]));

let currentPath = "";
const violations = [];
for (const line of diffParts.join("\n").split(/\r?\n/)) {
  const header = /^diff --git a\/(.+) b\/(.+)$/.exec(line);
  if (header) {
    currentPath = header[2];
    continue;
  }
  if (!line.startsWith("+") || line.startsWith("+++") || testPath.test(currentPath)) {
    continue;
  }
  if (line.includes("fixture") && /\.(unwrap|expect)\s*\(/i.test(line)) {
    continue;
  }
  for (const rule of rules) {
    if (rule.pattern.test(line)) violations.push(`[${rule.name}] ${line}`);
  }
}

if (violations.length > 0) {
  console.error(violations.slice(0, 40).join("\n"));
  console.error("New production guardrail violations detected. Use typed error paths instead.");
  process.exit(1);
}
