import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";

const modelsRoot = new URL("../../.github/codeql/extensions/relay-fixtures/", import.meta.url);
const targets = [
  "zenith_relay_core::test_support::temp_root",
  "zenith-relay::files::test_support::temp_root",
];

test("CodeQL models only the two private fixture-root return values", () => {
  const pack = Bun.YAML.parse(readFileSync(new URL("codeql-pack.yml", modelsRoot), "utf8"));
  expect(pack.library).toBe(true);
  expect(pack.extensionTargets).toEqual({ "codeql/rust-all": "^0.2.24" });
  expect(pack.dataExtensions).toEqual(["models/*.json"]);
  expect(pack.dependencies).toBeUndefined();

  const model = JSON.parse(readFileSync(new URL("models/fixture-roots.json", modelsRoot), "utf8"));
  expect(model).toEqual({
    extensions: [{
      addsTo: { pack: "codeql/rust-all", extensible: "barrierModel" },
      data: targets.map((target) => [target, "ReturnValue", "path-injection", "manual"]),
    }],
  });
});

test("modeled fixture wrappers remain test-only and reserve through the shared helper", () => {
  const source = (path) => readFileSync(new URL(`../../${path}`, import.meta.url), "utf8");
  expect(source("crates/relay-core/src/lib.rs")).toMatch(/#\[cfg\(test\)\]\s*mod test_support;/);
  expect(source("src-tauri/src/files.rs")).toMatch(/#\[cfg\(test\)\]\s*mod test_support;/);
  const core = source("crates/relay-core/src/test_support.rs");
  expect(core).toContain("SystemRandom::new().fill(&mut random).unwrap()");
  expect(core).toContain("fixture_root::create(label, &hex::encode(random))");
  expect(source("src-tauri/src/files/test_support.rs"))
    .toContain("fixture_root::create(label, &uuid::Uuid::new_v4().simple().to_string())");
});
