import { describe, expect, test } from "bun:test";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { basename, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { repoRoot, tauriInvocation } from "../lib/tauri-env.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
const read = (path) => readFileSync(resolve(root, path), "utf8");
const json = (path) => JSON.parse(read(path));

describe("Desktop and Hosted repository layout", () => {
  test("shared account services cannot depend on Desktop state or Tauri", () => {
    const core = Bun.TOML.parse(read("crates/relay-core/Cargo.toml"));
    expect(Object.keys(core.dependencies).some(name => name.startsWith("tauri"))).toBe(false);
    const visit = (directory) => {
      for (const entry of readdirSync(resolve(root, directory), { withFileTypes: true })) {
        const path = `${directory}/${entry.name}`;
        if (entry.isDirectory()) visit(path);
        else if (entry.name.endsWith(".rs")) {
          expect(read(path)).not.toMatch(/\b(?:tauri::|crate::local_pool::|crate::diagnostics::)/);
        }
      }
    };
    visit("crates/relay-core/src/providers/chatgpt/connector");
    expect(existsSync(resolve(root, "crates/relay-core/tests/host_management.rs"))).toBe(true);
    expect(existsSync(resolve(root, "desktop/src-tauri/src/local_pool/accounts/oauth.rs"))).toBe(false);
  });

  test("keeps independent packages and their lockfiles in product directories", () => {
    for (const directory of ["desktop/frontend", "hosted/web"]) {
      const pkg = json(`${directory}/package.json`);
      expect(pkg.scripts.build).toBeString();
      expect(existsSync(resolve(root, directory, "bun.lock"))).toBe(true);
    }
    expect(json("package.json").scripts["web:dev"]).toBe("bun run --cwd hosted/web dev");
  });

  test("native packaging resolves its frontend, license and shared core", () => {
    const nativeRoot = resolve(root, "desktop/src-tauri");
    const config = json("desktop/src-tauri/tauri.conf.json");
    for (const command of [config.build.beforeDevCommand, config.build.beforeBuildCommand]) {
      expect(resolve(nativeRoot, command.cwd)).toBe(resolve(root, "desktop/frontend"));
    }
    expect(resolve(nativeRoot, config.build.frontendDist)).toBe(resolve(root, "desktop/frontend/dist"));
    expect(resolve(nativeRoot, config.bundle.licenseFile)).toBe(resolve(root, "LICENSE"));
    const native = Bun.TOML.parse(read("desktop/src-tauri/Cargo.toml"));
    expect(resolve(nativeRoot, native.dependencies["zenith-relay-core"].path)).toBe(resolve(root, "crates/relay-core"));
  });

  test("shared catalog assets live inside the core package rather than the repository root", () => {
    const core = resolve(root, "crates/relay-core");
    let includes = 0;
    for (const path of [
      "src/model_metadata/order.rs", "src/model_metadata/protocol_policy.rs",
      "src/model_metadata/export.rs", "examples/export_model_catalog.rs",
    ]) {
      const source = resolve(core, path);
      for (const match of readFileSync(source, "utf8").matchAll(/include_str!\("([^"]+)"\)/g)) {
        const asset = resolve(dirname(source), match[1]);
        expect(asset).toBe(resolve(core, "catalog", basename(match[1])));
        expect(() => JSON.parse(readFileSync(asset, "utf8"))).not.toThrow();
        includes++;
      }
    }
    expect(includes).toBe(5);
  });

  test("build diagnostics and default exports stay under the local artifact directory", () => {
    expect(read("scripts/build/tauri-build.mjs"))
      .toContain('join(repoRoot(), ".build", "desktop", "desktop-build.log")');
    expect(read("crates/relay-core/examples/export_model_catalog.rs"))
      .toContain('.join("../../.build/output/model-catalog")');
  });

  test("Tauri wrappers use the desktop project without changing repository ownership", () => {
    expect(resolve(repoRoot())).toBe(resolve(root));
    const cli = resolve(root, "desktop/frontend/node_modules/@tauri-apps/cli/tauri.js");
    if (existsSync(cli)) expect(tauriInvocation(["--help"]).args[0]).toBe(cli);
    for (const wrapper of ["tauri-build", "tauri-dev"]) {
      expect(read(`scripts/build/${wrapper}.mjs`)).toContain('cwd: join(repoRoot(), "desktop")');
    }
  });

  test("Codex development actions resolve the relocated Desktop packages", () => {
    const environment = Bun.TOML.parse(read(".codex/environments/environment.toml"));
    const commands = environment.actions.map(action => action.command);
    expect(commands).toContain("bun run --cwd desktop/frontend check");
    expect(commands).toContain("bun run --cwd desktop/frontend test:unit");
    expect(commands).toContain("bun scripts/build/cargo.mjs check --manifest-path desktop/src-tauri/Cargo.toml --locked");
    expect(commands).toContain("bun run app:dev");
  });

  test("Hosted has no Tauri dependency and release packaging selects Desktop", () => {
    const hosted = json("hosted/web/package.json");
    expect(Object.keys({ ...hosted.dependencies, ...hosted.devDependencies }).some(name => name.startsWith("@tauri-apps/"))).toBe(false);
    const workflow = Bun.YAML.parse(read(".github/workflows/build.yml"));
    const bundle = workflow.jobs.build.steps.find(step => step.uses?.startsWith("tauri-apps/tauri-action@"));
    expect(bundle.with.projectPath).toBe("desktop");
  });
});
