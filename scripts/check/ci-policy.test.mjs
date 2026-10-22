import { describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const releasePush = "github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v')";

function workflow(name) {
  return Bun.YAML.parse(readFileSync(new URL(`../../.github/workflows/${name}`, import.meta.url), "utf8"));
}

function needs(job) {
  return Array.isArray(job.needs) ? job.needs : [job.needs];
}

test("Rust caches use the nested Cargo workspaces and their own target directories", () => {
  const cases = [
    ["build.yml", "check", ["src-tauri", "crates/relay-core"]],
    ["build.yml", "check-windows", ["src-tauri"]],
    ["build.yml", "build", ["src-tauri"]],
  ];
  for (const [file, jobName, directories] of cases) {
    const job = workflow(file).jobs[jobName];
    const cache = job.steps.find((step) => step.uses?.startsWith("Swatinem/rust-cache@"));
    const entries = cache?.with?.workspaces?.trim().split(/\r?\n/).map((entry) => entry.trim());
    expect(entries, `${file}: ${jobName}`).toEqual(
      directories.map((directory) => `${directory} -> target`),
    );
    for (const directory of directories) {
      expect(existsSync(new URL(`../../${directory}/Cargo.toml`, import.meta.url))).toBe(true);
    }
  }
});

describe("desktop CI", () => {
  const build = workflow("build.yml");

  test("retired server manifests and deployment are not part of supported builds", () => {
    for (const path of ["relay-server/Cargo.toml", ".github/workflows/relay-server.yml"]) {
      expect(existsSync(new URL(`../../${path}`, import.meta.url))).toBe(false);
    }
    expect(JSON.stringify(build)).not.toContain("relay-server/");
    const dependabot = Bun.YAML.parse(readFileSync(new URL("../../.github/dependabot.yml", import.meta.url), "utf8"));
    expect(dependabot.updates.filter((update) => update["package-ecosystem"] === "cargo")
      .map((update) => update.directory).sort()).toEqual(["/crates/relay-core", "/src-tauri"]);
  });

  test("an older pull request run is cancelled, but a tag or manual run is not", () => {
    expect(build.concurrency["cancel-in-progress"]).toBe("${{ github.event_name == 'pull_request' }}");
  });

  test("pull requests and main run checks, including the Windows Rust tests", () => {
    expect(build.jobs["check-windows"]["runs-on"]).toBe("windows-latest");
    expect(build.jobs["check-windows"].steps.at(-1).run)
      .toContain("cargo test --manifest-path src-tauri/Cargo.toml --locked -- --test-threads=1");
    expect(needs(build.jobs.build)).toEqual(["check", "check-windows", "check-web"]);
  });

  test("web preview is checked independently and must pass before packaging", () => {
    const web = build.jobs["check-web"];
    expect(web).toBeDefined();
    expect(web.defaults.run["working-directory"]).toBe("web");
    expect(web.if).toBeUndefined();
    expect(web["continue-on-error"]).toBeUndefined();
    for (const command of [
      "bun install --frozen-lockfile",
      "bun run test",
      "bun run build",
      "bun run test:e2e",
    ]) {
      const step = web.steps.find((step) => step.run === command);
      expect(step, command).toBeDefined();
      expect(step.if).toBeUndefined();
      expect(step["continue-on-error"]).toBeUndefined();
    }
    expect(web.steps.some((step) => step.run?.startsWith("bunx --no-install playwright install")
      && step.run.includes("chromium"))).toBe(true);
    expect(needs(build.jobs.build)).toContain("check-web");
  });

  test("installers are built only for a manual run or a pushed version tag", () => {
    expect(build.jobs.build.if).toBe(
      "github.event_name == 'workflow_dispatch' || (github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v'))",
    );
  });

  test("pool admission contention runs alongside the ordinary core suite", () => {
    const steps = build.jobs.check.steps;
    const core = steps.findIndex((step) => step.run === "cargo test --manifest-path crates/relay-core/Cargo.toml --locked");
    const load = steps.findIndex((step) => step.name === "Bounded pool admission under contention");
    expect(core).toBeGreaterThan(-1);
    expect(load).toBeGreaterThan(core);
    expect(steps[load].run).toBe("cargo test --manifest-path crates/relay-core/Cargo.toml --locked --lib large_mixed_queue_reaches_per_principal_limit_without_starvation -- --ignored");
    expect(steps[load].if).toBeUndefined();
    expect(steps[load]["continue-on-error"]).toBeUndefined();
  });

  test("a manual run cannot publish, even from a tag", () => {
    expect(build.jobs["publish-release"].if).toBe(releasePush);
    expect(build.jobs["publish-updater-manifest"].if).toBe(`${releasePush} && !contains(github.ref_name, '-')`);
    expect(build.jobs["publish-release"].if).not.toContain("workflow_dispatch");
  });
});

describe("agent guardrails", () => {
  test("checks large patches and untracked sources without changing the index", () => {
    const root = mkdtempSync(join(tmpdir(), "zenith-guardrails-test-"));
    const script = fileURLToPath(new URL("./check-agent-guardrails.mjs", import.meta.url));
    const env = { ...process.env };
    delete env.GITHUB_BASE_REF;
    try {
      for (const args of [
        ["init", "--quiet"],
        ["config", "user.name", "Synthetic fixture"],
        ["config", "user.email", "fixture@example.test"],
        ["config", "commit.gpgsign", "false"],
        ["commit", "--allow-empty", "--quiet", "-m", "fixture base"],
      ]) {
        const result = spawnSync("git", args, { cwd: root, encoding: "utf8", env, windowsHide: true });
        expect(result.status, result.stderr).toBe(0);
      }
      mkdirSync(join(root, "crates"));
      mkdirSync(join(root, "crates/tests"));
      const source = join(root, "crates/source.rs");
      const patch = "// Synthetic padding exercises the complete release diff.\n".repeat(25_000);
      expect(Buffer.byteLength(patch)).toBeGreaterThan(1_048_576);
      writeFileSync(source, patch);
      writeFileSync(join(root, "crates/tests/fixture.rs"), 'fn fixture() { panic!("synthetic"); }\n');
      writeFileSync(join(root, "crates/.gitignore"), "ignored.rs\n");
      writeFileSync(join(root, "crates/ignored.rs"), 'fn ignored() { panic!("synthetic"); }\n');
      const stage = spawnSync("git", ["add", "crates"], { cwd: root, env, windowsHide: true });
      expect(stage.status).toBe(0);
      const run = (cwd) => spawnSync(process.execPath, [script, "HEAD"], {
        cwd, env, encoding: "utf8", windowsHide: true,
      });
      const directories = [root, join(root, "crates")];
      for (const cwd of directories) {
        const clean = run(cwd);
        expect(clean.status, clean.stderr).toBe(0);
      }
      const untracked = join(root, "crates/new module.rs");
      writeFileSync(untracked, 'fn failed() { panic!("synthetic"); }\n');
      writeFileSync(join(root, "crates/tests/untracked.rs"), 'fn fixture() { panic!("synthetic"); }\n');
      const index = () => {
        const result = spawnSync("git", ["ls-files", "--stage"], {
          cwd: root, env, encoding: "utf8", windowsHide: true,
        });
        expect(result.status, result.stderr).toBe(0);
        return result.stdout;
      };
      const previousIndex = index();
      for (const cwd of directories) {
        const rejected = run(cwd);
        expect(rejected.status, rejected.stderr).toBe(1);
        expect(rejected.stderr).toContain("[panic] crates/new module.rs");
        expect(rejected.stderr).not.toContain("ignored.rs");
        expect(rejected.stderr).not.toContain("tests/untracked.rs");
        expect(index()).toBe(previousIndex);
      }
      writeFileSync(untracked, "fn safe() {}\n");
      for (const cwd of directories) {
        const clean = run(cwd);
        expect(clean.status, clean.stderr).toBe(0);
        expect(index()).toBe(previousIndex);
      }
      writeFileSync(source, patch + 'fn failed() { panic!("synthetic"); }\n');
      expect(spawnSync("git", ["add", "crates"], { cwd: root, env, windowsHide: true }).status).toBe(0);
      for (const cwd of directories) {
        const rejected = run(cwd);
        expect(rejected.status).toBe(1);
        expect(rejected.stderr).toContain("[panic] crates/source.rs");
        expect(rejected.stderr).not.toContain("ENOBUFS");
      }
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  }, 30_000);
});
