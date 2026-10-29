import { spawn, spawnSync } from "node:child_process";
import { createInterface } from "node:readline";
import { readCheckedText } from "../lib/checked-file.mjs";

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
// Lex unchanged context too: an added line may be inside an existing Rust
// comment or string. Only added lines are checked against the guard rules.
const fullContext = "--unified=2147483647";

if (requestedBase) {
  if (!hasCommit(requestedBase)) {
    throw new Error(`Guardrail check requires a valid base ref, got '${requestedBase}'.`);
  }
  diffs.push(["diff", fullContext, `${requestedBase}...HEAD`, "--", ...paths]);
} else if (process.env.GITHUB_BASE_REF) {
  git(["fetch", "--no-tags", "--quiet", "origin", process.env.GITHUB_BASE_REF]);
  diffs.push(["diff", fullContext, `origin/${process.env.GITHUB_BASE_REF}...HEAD`, "--", ...paths]);
} else if (hasCommit("HEAD^")) {
  diffs.push(["diff", fullContext, "HEAD^", "HEAD", "--", ...paths]);
}

diffs.push(["diff", fullContext, "--", ...paths]);
diffs.push(["diff", "--cached", fullContext, "--", ...paths]);

const violations = [];

function rustCodeLines() {
  let blockDepth = 0;
  let quoted = false;
  let rawEnd = "";
  return (line) => {
    let code = "";
    for (let index = 0; index < line.length;) {
      if (rawEnd) {
        const end = line.indexOf(rawEnd, index);
        if (end < 0) break;
        code += " ";
        index = end + rawEnd.length;
        rawEnd = "";
      } else if (blockDepth > 0) {
        if (line.startsWith("/*", index)) {
          blockDepth += 1;
          index += 2;
        } else if (line.startsWith("*/", index)) {
          blockDepth -= 1;
          index += 2;
        } else {
          index += 1;
        }
        code += " ";
      } else if (quoted) {
        if (line[index] === "\\") index += 2;
        else {
          if (line[index] === '"') quoted = false;
          index += 1;
        }
        code += " ";
      } else if (line.startsWith("//", index)) {
        break;
      } else if (line.startsWith("/*", index)) {
        blockDepth = 1;
        index += 2;
        code += " ";
      } else {
        const raw = /[rbc]/.test(line[index])
          ? /^(?:b|c)?r(#{0,255})"/.exec(line.slice(index))
          : null;
        const character = line[index] === "'"
          ? /^'(?:\\(?:u\{[\da-fA-F_]+\}|x[\da-fA-F]{2}|[^\r\n])|[^'\\\r\n])'/u.exec(line.slice(index))
          : null;
        if (raw) {
          rawEnd = `"${raw[1]}`;
          index += raw[0].length;
          code += " ";
        } else if (character) {
          index += character[0].length;
          code += " ";
        } else if (line[index] === '"') {
          quoted = true;
          index += 1;
          code += " ";
        } else {
          code += line[index];
          index += 1;
        }
      }
    }
    return code;
  };
}

function checkAddedLine(path, line, code) {
  if (testPath.test(path)) {
    return;
  }
  for (const rule of rules) {
    if (rule.pattern.test(code) && violations.length < 40) {
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
  let codeLine = rustCodeLines();
  let inHunk = false;
  try {
    lines.on("line", (line) => {
      const header = /^diff --git a\/(.+) b\/(.+)$/.exec(line);
      if (header) {
        currentPath = header[2];
        codeLine = rustCodeLines();
        inHunk = false;
        return;
      }
      if (line.startsWith("@@ ")) {
        inHunk = true;
        return;
      }
      if (!inHunk || (line[0] !== "+" && line[0] !== " ")) return;
      const code = codeLine(line.slice(1));
      if (line[0] === "+") checkAddedLine(currentPath, line, code);
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
  if (!path.endsWith(".rs") || testPath.test(path)) {
    continue;
  }
  const text = readCheckedText(path);
  if (text === null) continue;
  const codeLine = rustCodeLines();
  for (const line of text.split(/\r?\n/)) {
    checkAddedLine(path, `+${line}`, codeLine(line));
  }
}

if (violations.length > 0) {
  console.error(violations.slice(0, 40).join("\n"));
  console.error("New production guardrail violations detected. Use typed error paths instead.");
  process.exit(1);
}
