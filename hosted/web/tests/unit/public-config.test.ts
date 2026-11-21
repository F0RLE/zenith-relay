import { expect, test } from "bun:test";
import { readPublicConfig } from "../../src/api/publicConfig";

const meta = { api_version: "1", request_id: "aec1d95e-748c-4c09-9d76-844ed9471235" };
const data = { display_name: "Example", login_methods: [], registration_open: false, studio_enabled: false };

test("the setup client projects branding only, never trusts authority from the response", () => {
  expect(readPublicConfig({ meta, data: { ...data, admin: true } })).toEqual({ displayName: "Example" });
  for (const payload of [
    {}, { data }, { meta }, { meta: { ...meta, api_version: "2" }, data }, { meta: { ...meta, request_id: null }, data },
    ...[{ display_name: " " }, { display_name: "x".repeat(65) }, { display_name: "\x00" },
      { login_methods: ["fixture"] }, { registration_open: true }, { studio_enabled: true }].map(update => ({ meta, data: { ...data, ...update } })),
  ]) expect(() => readPublicConfig(payload)).toThrow();
});

test("only named, normalized, credential-free HTTPS pages can be published as links", () => {
  const parse = (policy_links: unknown) => readPublicConfig({ meta, data: { ...data, policy_links } });
  const links = { terms: "https://policy.example/terms", support: "https://help.example/" };
  expect(parse(links).policyLinks).toEqual(links);
  for (const value of [null, [], { secret: "unrecognized" }, ...[
    "javascript:alert(1)", "http://policy.example/", "//policy.example/privacy", "https://user@policy.example/",
    "https://policy.example/?secret=value", "https://policy.example/#frag", "https://policy.example/\\path",
  ].map(privacy => ({ privacy }))]) expect(() => parse(value)).toThrow();
});

test("a configured identity method is not open public registration", () => {
  const provider = "991555b9-0124-4d25-9520-06c22ec36e1e";
  const parse = (methods: unknown[]) => readPublicConfig({ meta, data: { ...data, login_methods: methods } });
  expect(parse([provider])).toEqual({ displayName: "Example", loginProvider: provider });
  for (const methods of [[provider, provider], [null], ["//login.example"], [{}]])
    expect(() => parse(methods)).toThrow();
});
