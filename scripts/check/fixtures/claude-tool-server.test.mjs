import { describe, expect, test } from "bun:test";
import { createFixtureProtocol, FIXTURE_RESULT } from "./claude-tool-server.mjs";

const rpc = (method, params, id = 1) => ({ jsonrpc: "2.0", id, method, params });
async function ready(handle) {
  const reply = await handle(rpc("initialize", { protocolVersion: "2025-11-25" }));
  expect(reply.result.protocolVersion).toBe("2025-11-25");
  await handle({ jsonrpc: "2.0", method: "notifications/initialized" });
}

describe("synthetic MCP tool fixture", () => {
  test("requires initialization and the initialized notification before exposing tools", async () => {
    const handle = createFixtureProtocol();
    expect((await handle(rpc("tools/list"))).error.code).toBe(-32600);
    expect((await handle(rpc("ping"))).result).toEqual({});
    await handle(rpc("initialize", { protocolVersion: "2026-07-28" }));
    expect((await handle(rpc("tools/list"))).error.code).toBe(-32600);
    await handle({ jsonrpc: "2.0", method: "notifications/initialized" });
    expect((await handle(rpc("tools/list"))).result.tools.map(tool => tool.name)).toEqual(["lookup"]);
    expect((await handle(rpc("initialize", { protocolVersion: "2025-11-25" }))).error.code).toBe(-32602);
  });

  test("returns the deterministic result only after exactly one successful audit callback", async () => {
    let calls = 0;
    const handle = createFixtureProtocol(async () => { calls++; });
    await ready(handle);
    const call = rpc("tools/call", { name: "lookup", arguments: { value: "synthetic" } });
    const first = await handle(call);
    expect(first.result).toEqual({ content: [{ type: "text", text: FIXTURE_RESULT }], isError: false });
    expect(calls).toBe(1);
    expect((await handle(call)).result.isError).toBeTrue();
    expect(calls).toBe(1);
  });

  test("rejects other tools and arbitrary arguments without executing a callback", async () => {
    let calls = 0;
    const handle = createFixtureProtocol(async () => { calls++; });
    await ready(handle);
    for (const params of [
      { name: "shell", arguments: { value: "synthetic" } },
      { name: "lookup", arguments: { value: "private" } },
      { name: "lookup", arguments: { value: "synthetic", path: "private" } },
      { name: "lookup", arguments: [] },
    ]) expect((await handle(rpc("tools/call", params))).error.code).toBe(-32602);
    expect((await handle(rpc("resources/read", { uri: "file:///private" }))).error.code).toBe(-32601);
    expect(calls).toBe(0);
  });

  test("does not turn audit failure into a successful tool result", async () => {
    const handle = createFixtureProtocol(async () => { throw new Error("synthetic audit outage"); });
    await ready(handle);
    const result = await handle(rpc("tools/call", { name: "lookup", arguments: { value: "synthetic" } }));
    expect(result.result.isError).toBeTrue();
    expect(JSON.stringify(result)).not.toContain("synthetic audit outage");
  });
});
