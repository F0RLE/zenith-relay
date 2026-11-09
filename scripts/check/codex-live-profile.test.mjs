import { expect, test } from "bun:test";
import { mkdtemp, realpath, rm, writeFile } from "node:fs/promises";
import { isAbsolute, join, relative, sep } from "node:path";
import { tmpdir } from "node:os";
import { liveCodexProfile } from "./check-codex-wire.mjs";

const key = "synthetic-private-profile-key";
const models = [{ slug: "synthetic-model", base_instructions: "Synthetic instructions",
  supported_reasoning_levels: [{ effort: "high" }] }, { slug: "other-model" }];

async function withProfile(run) {
  const root = await realpath(await mkdtemp(join(tmpdir(), "relay-codex-profile-test-")));
  const profile = join(root, "config.toml"), catalog = join(root, "catalog.json");
  const save = async (base = "http://127.0.0.1:14998/v1", rows = models) => {
    await writeFile(catalog, JSON.stringify({ models: rows }));
    await writeFile(profile, `model_provider="fixture"\nmodel_catalog_json=${JSON.stringify(catalog)}\n[model_providers.fixture]\nbase_url=${JSON.stringify(base)}\nexperimental_bearer_token=${JSON.stringify(key)}\n`);
  };
  try {
    await save();
    await run({ profile, catalog, save, options: { profile, model: "synthetic-model", effort: "high", allowProviderRequests: true } });
  } finally {
    const resolved = await realpath(root), location = relative(await realpath(tmpdir()), resolved);
    if (resolved !== root || isAbsolute(location) || location === ".." || location.startsWith(`..${sep}`)
      || !location.startsWith("relay-codex-profile-test-")) throw new Error("Profile test cleanup containment failed");
    await rm(resolved, { recursive: true });
  }
}

test("live profile access requires consent and explicit model and effort", async () => {
  await expect(liveCodexProfile({})).rejects.toThrow("--allow-provider-requests");
});

test("live selection reads one declared row without exposing the remaining inventory", async () => {
  await withProfile(async ({ options }) => {
    const selected = await liveCodexProfile(options);
    expect(selected.gatewayOrigin).toBe("http://127.0.0.1:14998");
    expect(selected.catalog.models).toEqual([models[0]]);
    expect(selected.live.key).toBe(key);
    expect(selected.effort).toBe("high");
  });
});

test("live selection does not substitute an undeclared model or effort", async () => {
  await withProfile(async ({ options, save }) => {
    await expect(liveCodexProfile({ ...options, model: "missing-model" })).rejects.toThrow("exactly the selected model and effort");
    await expect(liveCodexProfile({ ...options, effort: "low" })).rejects.toThrow("exactly the selected model and effort");
    await save(undefined, [models[0], models[0]]);
    await expect(liveCodexProfile(options)).rejects.toThrow("exactly the selected model and effort");
  });
});

test("live profile credentials cannot be forwarded to a remote or credential-bearing URL", async () => {
  await withProfile(async ({ options, save }) => {
    for (const base of ["https://provider.invalid/v1", `http://user:${key}@127.0.0.1:14998/v1`, "http://127.0.0.1:14998/other"]) {
      await save(base);
      await expect(liveCodexProfile(options)).rejects.toThrow();
    }
  });
});

test("invalid profile diagnostics contain no source text or credentials", async () => {
  await withProfile(async ({ profile, options }) => {
    await writeFile(profile, `invalid profile ${key}`);
    try { await liveCodexProfile(options); throw new Error("Expected invalid profile rejection"); }
    catch (error) {
      expect(error.message).toBe("Cannot read the selected Codex profile and generated catalog safely");
      expect(error.message).not.toContain(key);
      expect(error.message).not.toContain(profile);
    }
  });
});

test("oversized profile and catalog reads fail without exposing their content", async () => {
  await withProfile(async ({ profile, catalog, options, save }) => {
    await writeFile(profile, " ".repeat(512 * 1024 + 1));
    await expect(liveCodexProfile(options)).rejects.toThrow("Cannot read the selected Codex profile and generated catalog safely");
    await save();
    await writeFile(catalog, " ".repeat(16 * 1024 * 1024 + 1));
    await expect(liveCodexProfile(options)).rejects.toThrow("Cannot read the selected Codex profile and generated catalog safely");
  });
});
