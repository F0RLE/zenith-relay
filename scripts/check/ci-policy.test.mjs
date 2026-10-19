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
    ["relay-server.yml", "check", ["relay-server"]],
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

  test("an older pull request run is cancelled, but a tag or manual run is not", () => {
    expect(build.concurrency["cancel-in-progress"]).toBe("${{ github.event_name == 'pull_request' }}");
  });

  test("pull requests and main run checks, including the Windows Rust tests", () => {
    expect(build.jobs["check-windows"]["runs-on"]).toBe("windows-latest");
    expect(build.jobs["check-windows"].steps.at(-1).run)
      .toContain("cargo test --manifest-path src-tauri/Cargo.toml --locked -- --test-threads=1");
    expect(needs(build.jobs.build)).toEqual(["check", "check-windows"]);
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

describe("server CI", () => {
  const server = workflow("relay-server.yml");

  test("the retired product-mode branch is not a trigger", () => {
    expect(server.on.push.branches).toEqual(["main"]);
    expect(JSON.stringify(server.on)).not.toContain("relay/local-pool-product-modes");
  });

  test("package publishing is limited to the tag publish job", () => {
    expect(server.permissions).toEqual({ contents: "read" });
    expect(server.jobs.smoke.permissions).toBeUndefined();
    expect(server.jobs.check.permissions).toBeUndefined();
    expect(server.jobs.publish.permissions).toEqual({ contents: "read", packages: "write" });
    expect(server.jobs.publish.if).toBe(releasePush);
    expect(needs(server.jobs.publish)).toEqual(["smoke"]);
    expect(JSON.stringify(server.jobs.smoke)).not.toContain("docker/login-action");
    expect(JSON.stringify(server.jobs.publish.steps)).not.toContain("edge-");
  });

  test("only manual runs retain the smoke-tested image without publishing it", () => {
    const exportImage = server.jobs.smoke.steps.find((step) => step.name === "Export checked server image");
    const upload = server.jobs.smoke.steps.find((step) => step.uses?.startsWith("actions/upload-artifact@"));
    expect(exportImage.if).toBe("github.event_name == 'workflow_dispatch'");
    expect(exportImage.run).toContain('docker save "zenith-relay-server:$GITHUB_SHA"');
    expect(exportImage.run).toContain("sha256sum");
    expect(upload.if).toBe(exportImage.if);
    expect(upload.with["if-no-files-found"]).toBe("error");
    expect(server.jobs.smoke.steps.indexOf(exportImage))
      .toBeGreaterThan(server.jobs.smoke.steps.findIndex((step) => step.name === "Smoke-test server container"));
  });
});

describe("agent guardrails", () => {
  test("checks a large patch from root and nested directories and excludes test files", () => {
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
