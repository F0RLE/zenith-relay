import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";

const read = path => readFileSync(new URL(`../../${path}`, import.meta.url), "utf8");

test("CodeQL trusts only validated endpoint URLs and explicitly selected diagnostic roots", () => {
  const root = ".github/codeql/extensions/relay-boundaries/";
  expect(Bun.YAML.parse(read(`${root}codeql-pack.yml`)).extensionTargets)
    .toEqual({ "codeql/rust-all": "^0.2.25" });
  const targets = [
    ["zenith_relay_core::runtime::support::normalized_responses_url", "ReturnValue", "request-url", "manual"],
    ["zenith_relay_core::runtime::support::normalized_responses_url", "ReturnValue.Field[core::result::Result::Ok(0)]", "request-url", "manual"],
    ["zenith-relay::local_pool::accounts::proxy::diagnose::diagnostic_root", "ReturnValue", "path-injection", "manual"],
    ["zenith-relay::local_pool::accounts::proxy::diagnose::diagnostic_root", "ReturnValue.Field[core::result::Result::Ok(0)].Field[core::option::Option::Some(0)]", "path-injection", "manual"],
  ];
  expect(JSON.parse(read(`${root}models/validated-inputs.json`))).toEqual({ extensions: [{
    addsTo: { pack: "codeql/rust-all", extensible: "barrierModel" }, data: targets,
  }] });
  const endpoints = read("crates/relay-core/src/runtime/support.rs");
  expect(endpoints).toContain('url.scheme() == "http" && !is_loopback_url(&url)');
  expect(endpoints).toContain("url_has_userinfo(&url) || url.query().is_some() || url.fragment().is_some()");
  expect(read("crates/relay-core/src/runtime.rs"))
    .toContain("support::normalized_responses_url(self.upstream_url.as_str())");
  const diagnostic = read("desktop/src-tauri/src/local_pool/accounts/proxy/diagnose.rs");
  expect(diagnostic).toContain("fn diagnostic_root(args:");
  expect(diagnostic).not.toContain("pub fn diagnostic_root");
  expect(diagnostic).toContain("args.len() != 3");
  expect(diagnostic).toContain("Some(FLAG | AUTHORIZATION_FLAG)");
  expect(diagnostic).toContain("!Path::new(&args[2]).is_absolute()");
  expect(diagnostic).toContain("part == std::path::Component::ParentDir");
  expect(diagnostic).toContain("ProxyPool::load_read_only(&vault_root)");
});
