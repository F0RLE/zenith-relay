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
    ["build.yml", "check-runtime", ["src-tauri", "crates/relay-core"]],
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

  test("documentation checks use locked dependencies and cannot be skipped silently", () => {
    const steps = build.jobs["check-runtime"].steps;
    const install = steps.findIndex((step) => step.run === "bun install --frozen-lockfile"
      && step["working-directory"] === "src");
    const scripts = steps.findIndex((step) => step.run === "bun test ./scripts/check ./scripts/lib ./scripts/maintenance ./scripts/build");
    const docs = steps.findIndex((step) => step.run === "bun ./scripts/check/check-docs.mjs");
    expect(install).toBeGreaterThan(-1);
    expect(scripts).toBeGreaterThan(install);
    expect(docs).toBeGreaterThan(install);
    expect(steps[docs].if).toBeUndefined();
    expect(steps[docs]["continue-on-error"]).toBeUndefined();
  });

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
    expect(build.jobs.build.needs).toBeUndefined();
    expect(needs(build.jobs["publish-release"])).toEqual(["check", "check-windows", "check-web", "build"]);
  });

  test("web preview is checked independently and must pass before publication", () => {
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
    expect(needs(build.jobs["publish-release"])).toContain("check-web");
  });

  test("browser shards run independently without cancelling the remaining coverage", () => {
    const ui = build.jobs["check-ui"];
    expect(ui.needs).toBeUndefined();
    expect(ui.if).toBeUndefined();
    expect(ui["continue-on-error"]).toBeUndefined();
    expect(ui.strategy["fail-fast"]).toBe(false);
    expect(ui.strategy.matrix.shard).toEqual([1, 2, 3, 4]);
    expect(ui.defaults.run["working-directory"]).toBe("src");
    const run = ui.steps.find((step) => step.name === "Run browser tests");
    expect(run.run).toBe("bun run test:e2e --shard=${{ matrix.shard }}/${{ strategy.job-total }}");
    expect(run.if).toBeUndefined();
    expect(run["continue-on-error"]).toBeUndefined();
    expect(build.jobs["check-runtime"].steps.some((step) => step.run === "bun run test:e2e")).toBe(false);
  });

  test("the existing Check status fails closed on failed, skipped or cancelled dependencies", () => {
    const gate = build.jobs.check;
    expect(gate.name).toBe("Check");
    expect(needs(gate)).toEqual(["check-runtime", "check-ui"]);
    expect(gate.if).toBe("always()");
    expect(gate["continue-on-error"]).toBeUndefined();
    expect(gate.steps).toHaveLength(1);
    const step = gate.steps[0];
    expect(step.shell).toBe("pwsh");
    expect(step.env).toEqual({
      RUNTIME_RESULT: "${{ needs.check-runtime.result }}",
      UI_RESULT: "${{ needs.check-ui.result }}",
    });
    expect(step.run.trim()).toBe([
      "if ($env:RUNTIME_RESULT -ne 'success' -or $env:UI_RESULT -ne 'success') {",
      "  exit 1",
      "}",
    ].join("\n"));
    expect(step.if).toBeUndefined();
    expect(step["continue-on-error"]).toBeUndefined();
  });

  test("installers are built only for a manual run or a pushed version tag", () => {
    expect(build.jobs.build.if).toBe(
      "github.event_name == 'workflow_dispatch' || (github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v'))",
    );
  });

  test("Linux checks use the canonical archive for direct and mirror-list APT sources", () => {
    for (const jobName of ["check-runtime", "check-ui", "check-web"]) {
      const steps = build.jobs[jobName].steps;
      const mirrorIndex = steps.findIndex((step) => step.run?.includes("sudo sed -i -E"));
      expect(mirrorIndex, jobName).toBeGreaterThan(-1);
      const script = steps[mirrorIndex].run;
      expect(script).toContain("/etc/apt/sources.list.d/ubuntu.sources");
      const replacements = [...script.matchAll(/s\|([^|]+)\|([^|]+)\|g/g)];
      expect(replacements).toHaveLength(2);
      for (const source of [
        "http://azure.archive.ubuntu.com/ubuntu/",
        "https://azure.archive.ubuntu.com/ubuntu/",
        "mirror+file:/etc/apt/apt-mirrors.txt",
      ]) {
        const rewritten = replacements.reduce((value, [, pattern, replacement]) =>
          value.replace(new RegExp(pattern, "g"), replacement), `URIs: ${source}`);
        expect(rewritten).toBe("URIs: https://archive.ubuntu.com/ubuntu/");
      }
      expect(steps[mirrorIndex]["continue-on-error"]).toBeUndefined();
      if (jobName !== "check-runtime") {
        const browser = steps.findIndex((step) => step.run?.startsWith("bunx --no-install playwright install"));
        expect(browser).toBeGreaterThan(mirrorIndex);
        expect(steps[browser]["timeout-minutes"]).toBe(10);
      }
    }
  });

  test("pool admission contention runs alongside the ordinary core suite", () => {
    const steps = build.jobs["check-runtime"].steps;
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
