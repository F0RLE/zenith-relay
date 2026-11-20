import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";

const read = path => readFileSync(new URL(`../../${path}`, import.meta.url), "utf8");

test("deserialization models name only the two rejecting size guards", () => {
  const root = ".github/codeql/extensions/relay-input-bounds/";
  const pack = Bun.YAML.parse(read(`${root}codeql-pack.yml`));
  expect(pack.library).toBe(true);
  expect(pack.extensionTargets).toEqual({ "codeql/rust-all": "^0.2.25" });
  expect(pack.dataExtensions).toEqual(["models/*.json"]);
  const targets = ["zenith_relay_core::bounded_json::checked_input", "zenith_relay_hosted::config::checked_text"];
  expect(JSON.parse(read(`${root}models/size-guards.json`))).toEqual({ extensions: [{
    addsTo: { pack: "codeql/rust-all", extensible: "barrierModel" },
    data: targets.flatMap(target => ["ReturnValue", "ReturnValue.Field[core::result::Result::Ok(0)]"]
      .map(output => [target, output, "unsafe-deserialization", "manual"])),
  }] });
  const json = read("crates/relay-core/src/bounded_json.rs");
  expect(json).toContain("const MAX_BYTES: usize = 4 * 1024 * 1024;");
  expect(json).toMatch(/if input\.len\(\) > LIMIT \{\s*return Err/);
  expect(json).toContain("from_slice_with_limit::<T, MAX_BYTES>(input)");
  expect(json).toContain("serde_json::from_slice(checked_input::<LIMIT>(input)?)");
  expect(json).not.toContain("disable_recursion_limit(");
  const config = read("hosted/server/src/config.rs");
  expect(config).toMatch(/fn checked_text[\s\S]*?if text\.len\(\) > 65_536 \{\s*return Err/);
  expect(config).toContain("toml::from_str(checked_text(text)?)");
});
