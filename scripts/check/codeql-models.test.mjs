import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";

const modelsRoot = new URL("../../.github/codeql/extensions/relay-fixtures/", import.meta.url);
const targets = [
  ["zenith_relay_core::test_support::temp_root", "ReturnValue"],
  ["zenith-relay::files::test_support::temp_root", "ReturnValue"],
  ["zenith-relay::local_pool::profiles::codex::tests::binding_safety::direct_reads::fifo_child_root", "ReturnValue.Field[core::option::Option::Some(0)]"],
  ["zenith_relay_core::model_metadata::memory::benchmark_tests::reference_catalog_path", "ReturnValue"],
  ["zenith_relay_core::pricing::memory::benchmark_tests::reference_catalog_directory", "ReturnValue"],
];

test("CodeQL models only exact test-only trusted-path return values", () => {
  const pack = Bun.YAML.parse(readFileSync(new URL("codeql-pack.yml", modelsRoot), "utf8"));
  expect(pack.library).toBe(true);
  expect(pack.extensionTargets).toEqual({ "codeql/rust-all": "^0.2.24" });
  expect(pack.dataExtensions).toEqual(["models/*.json"]);
  expect(pack.dependencies).toBeUndefined();

  const model = JSON.parse(readFileSync(new URL("models/fixture-roots.json", modelsRoot), "utf8"));
  expect(model).toEqual({
    extensions: [{
      addsTo: { pack: "codeql/rust-all", extensible: "barrierModel" },
      data: targets.map(([target, output]) => [target, output, "path-injection", "manual"]),
    }],
  });
});

test("operator-path models remain limited to ignored read-only public-catalog probes", () => {
  for (const module of ["model_metadata", "pricing"]) {
    const memory = readFileSync(new URL(`../../crates/relay-core/src/${module}/memory.rs`, import.meta.url), "utf8");
    expect(memory).toMatch(/#\[cfg\(test\)\]\s*mod benchmark_tests;/);
    const probe = readFileSync(new URL(`../../crates/relay-core/src/${module}/memory/benchmark_tests.rs`, import.meta.url), "utf8");
    expect(probe).toContain("#[ignore = \"read-only local");
    expect(probe).toMatch(/fn reference_catalog_(path|directory)\(\) -> std::path::PathBuf/);
    expect(probe).not.toMatch(/\.(refresh|write|remove|save|persist)\(/);
  }
});

test("the modeled FIFO channel validates its parent's private fixture before use", () => {
  const source = readFileSync(new URL("../../src-tauri/src/local_pool/profiles/codex/tests/binding_safety/direct_reads.rs", import.meta.url), "utf8");
  expect(source).toMatch(/#\[cfg\(unix\)\]\s*fn fifo_child_root\(\) -> Option<PathBuf>/);
  expect(source).toContain("assert_eq!(root.parent(), Some(temporary_directory.as_path()))");
  expect(source).toContain('.strip_prefix("zenith-relay-profile-backup-fifo-")');
  expect(source).toContain("nonce.len() == 32 && nonce.bytes().all(|byte| byte.is_ascii_hexdigit())");
  expect(source).toContain("metadata.is_dir() && !metadata.file_type().is_symlink()");
  expect(source).toContain("metadata.permissions().mode() & 0o777, 0o700");
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
