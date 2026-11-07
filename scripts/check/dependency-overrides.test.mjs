import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";

const readToml = (path) => Bun.TOML.parse(readFileSync(new URL(`../../${path}`, import.meta.url), "utf8"));
const desktop = readToml("src-tauri/Cargo.toml");
const packages = readToml("src-tauri/Cargo.lock").package;

test("Git dependencies use immutable, credential-free revisions matched by the lockfile", () => {
  for (const pkg of packages.filter((pkg) => pkg.source?.startsWith("git+"))) {
    const source = new URL(pkg.source.slice(4));
    const revision = source.searchParams.get("rev");
    expect(revision, pkg.name).toMatch(/^[a-f0-9]{40}$/);
    expect(source.hash.slice(1), pkg.name).toBe(revision);
    expect(source.searchParams.has("branch"), pkg.name).toBe(false);
    expect(source.searchParams.has("tag"), pkg.name).toBe(false);
    expect(source.username, pkg.name).toBe("");
    expect(source.password, pkg.name).toBe("");
  }
  for (const [name, patch] of Object.entries(desktop.patch?.["crates-io"] ?? {})) {
    if (!patch.git) continue;
    expect(patch.rev, name).toMatch(/^[a-f0-9]{40}$/);
    const resolved = packages.filter((pkg) => pkg.name === name);
    expect(resolved, name).toHaveLength(1);
    expect(resolved[0].source, name).toBe(`git+${patch.git}?rev=${patch.rev}#${patch.rev}`);
  }
});

test("localization build configuration uses the maintained TOML parser", () => {
  expect(packages.some((pkg) => pkg.name === "basic-toml")).toBe(false);
  const config = packages.find((pkg) => pkg.name === "i18n-config");
  expect(config).toBeDefined();
  expect(config.dependencies.some((name) => name.split(" ")[0] === "toml")).toBe(true);
});

test("the compiler compatibility override keeps its attribute macro on the same source", () => {
  if (!desktop.patch?.["crates-io"]?.["proc-macro-error2"]) return;
  const implementation = packages.find((pkg) => pkg.name === "proc-macro-error2");
  const attribute = packages.find((pkg) => pkg.name === "proc-macro-error-attr2");
  expect(implementation).toBeDefined();
  expect(attribute).toBeDefined();
  expect(attribute.source).toBe(implementation.source);
});
