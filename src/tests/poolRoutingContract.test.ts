import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import type { PoolRoutingMode } from "../src/features/relay/api/types";

type PolicySchema = {
  properties: { version: { enum: number[] }; mode: { enum: string[] } };
  oneOf: Array<{ properties: { version: { const: number }; mode: { enum: string[] } } }>;
};

const document = Bun.YAML.parse(readFileSync(new URL("../../relay-server/openapi.yaml", import.meta.url), "utf8")) as {
  components: { schemas: { PoolRoutingPolicy: PolicySchema; Capabilities: { required: string[]; properties: Record<string, unknown> } } };
};
const schema = document.components.schemas.PoolRoutingPolicy;

describe("pool routing OpenAPI contract", () => {
  test("uses the actual capability wire fields instead of a fictional protocol range", () => {
    const capabilities = document.components.schemas.Capabilities;
    const fields = [
      "protocolVersion", "compatibilityMinClient", "serverName", "serverId", "identityFingerprint", "serverManagedByUser",
      "features", "supportedWireApis", "supportsAccounts", "supportsSources", "supportsQuota", "supportsUsage",
      "supportsLocalGateway", "supportsProfileAttach", "supportsWakeTasks",
    ];
    expect([...capabilities.required].sort()).toEqual([...fields].sort());
    expect(Object.keys(capabilities.properties).sort()).toEqual([...fields].sort());
  });

  test("advertises every runtime mode and keeps legacy-only modes out of newer policies", () => {
    const modes = ["smart", "automatic", "in_order", "round_robin", "weighted_round_robin", "failover"] as const;
    const allowed: Record<number, readonly string[]> = {
      1: ["smart", "in_order", "round_robin"],
      2: ["automatic", "in_order", "round_robin"],
      3: ["automatic", "in_order", "round_robin", "weighted_round_robin", "failover"] satisfies PoolRoutingMode[],
    };
    expect([...schema.properties.version.enum].sort()).toEqual([1, 2, 3]);
    expect([...schema.properties.mode.enum].sort()).toEqual([...modes].sort());
    for (const version of [0, 1, 2, 3, 4]) {
      for (const mode of [...modes, "unknown"]) {
        const matches = schema.oneOf.filter((branch) => branch.properties.version.const === version && branch.properties.mode.enum.includes(mode));
        expect(matches.length).toBe(allowed[version]?.includes(mode) ? 1 : 0);
      }
    }
  });
});
