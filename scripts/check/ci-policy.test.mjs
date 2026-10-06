import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";

const releasePush = "github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v')";

function workflow(name) {
  return Bun.YAML.parse(readFileSync(new URL(`../../.github/workflows/${name}`, import.meta.url), "utf8"));
}

function needs(job) {
  return Array.isArray(job.needs) ? job.needs : [job.needs];
}

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
});
