import { describe, expect, test } from "bun:test";
import { stat } from "node:fs/promises";
import { createServer, request as httpRequest } from "node:http";
import { qualifyCodexWire, isolatedCodexEnvironment } from "./check-codex-wire.mjs";
import { FIXTURE_TOOL, FIXTURE_RESULT } from "./fixtures/claude-tool-server.mjs";

const catalog = { models: [{ slug: "synthetic-model", base_instructions: "Synthetic model catalog instruction.", supported_reasoning_levels: [{ effort: "low" }] }] };
const help = "--strict-config --ignore-user-config --ignore-rules --ephemeral --sandbox --json --skip-git-repo-check";
const child = (stdout = "") => ({ code: 0, stdout, stderr: "", failure: null });
const completed = (text = "relay-codex-contract-ok") => child([
  { type: "item.completed", item: { type: "agent_message", text } },
  { type: "turn.completed", usage: { input_tokens: 10, output_tokens: 10 } },
].map(value => JSON.stringify(value)).join("\n"));

function fakeClient(calls, { noCycle = false, badResult = false, metadataPart = false,
  fallbackCatalog = false, malformedFirst = false, diagnostics = false, outbound = false, liveAcknowledgement = false } = {}) {
  return async input => {
    calls.push(input);
    if (input.args[0] === "--version") return child("codex-cli 0.1-fixture\n");
    if (input.args.includes("--help")) return child(help);
    const model = input.args[input.args.indexOf("--model") + 1];
    const effort = JSON.parse(input.args.find(value => value.startsWith("model_reasoning_effort=")).slice("model_reasoning_effort=".length));
    const origin = input.env.HTTP_PROXY;
    const token = input.args.join(" ").match(/RELAY_TOOL_FIXTURE_TOKEN="([a-f0-9]{64})"/)[1];
    const headers = { authorization: `Bearer ${input.env.RELAY_CODEX_FIXTURE_KEY}`, "content-type": "application/json" };
    const body = { model, stream: true, instructions: fallbackCatalog ? "Unrelated client default" : catalog.models[0].base_instructions, reasoning: { effort },
      tools: [{ type: "function", name: FIXTURE_TOOL }], input: [{ role: "user", content: "Synthetic task" }] };
    if (noCycle) return completed();
    if (outbound) await new Promise(resolve => {
      const request = httpRequest(origin, { method: "CONNECT", path: "provider.invalid:443" });
      request.once("error", resolve);
      request.once("connect", (_response, socket) => { socket.destroy(); resolve(); });
      request.end();
    });
    if (malformedFirst) {
      const failed = await fetch(`${origin}/v1/responses`, { method: "POST", headers, body: "{" });
      expect(failed.status).toBe(400);
      await failed.text();
    }
    const first = await fetch(`${origin}/v1/responses`, { method: "POST", headers, body: JSON.stringify(body) });
    if (fallbackCatalog || malformedFirst) { expect(first.status).toBe(400); await first.text(); return completed(); }
    expect(first.status).toBe(200);
    const frames = (await first.text()).split("\n").filter(line => line.startsWith("data: ")).map(line => JSON.parse(line.slice(6)));
    const response = frames.find(frame => frame.type === "response.completed").response;
    const call = response.output[0];
    const audit = await fetch(`${origin}/fixture/tool-call`, { method: "POST", headers: { "x-relay-fixture-token": token } });
    expect(audit.status).toBe(204);
    const output = metadataPart ? [{ type: "input_text", text: FIXTURE_RESULT }, { type: "input_text", text: "Synthetic client execution metadata" }]
      : badResult ? "wrong fixture result" : FIXTURE_RESULT;
    body.input = [call, { type: "function_call_output", call_id: call.call_id, output }];
    const second = await fetch(`${origin}/v1/responses`, { method: "POST", headers, body: JSON.stringify(body) });
    await second.text();
    const result = completed(liveAcknowledgement ? FIXTURE_RESULT : undefined);
    if (diagnostics) {
      result.code = 1;
      result.stderr = `${input.env.RELAY_CODEX_FIXTURE_KEY} ${token} ${input.cwd} ${JSON.stringify(input.cwd)}`;
    }
    return result;
  };
}

describe("isolated Codex wire qualification", () => {
  test("inherits platform basics rather than user accounts, flags or proxy settings", () => {
    const env = isolatedCodexEnvironment("fixture-root", "http://127.0.0.1:1", {
      PATH: "fixture-path", OPENAI_API_KEY: "private-key", CODEX_HOME: "private-profile",
      HTTP_PROXY: "private-proxy", CODEX_CLI_MODEL: "private-model", ZENITH_ACCOUNT: "private-account",
    });
    expect(env.PATH).toBe("fixture-path");
    for (const name of ["OPENAI_API_KEY", "CODEX_CLI_MODEL", "ZENITH_ACCOUNT"]) expect(env[name]).toBeUndefined();
    expect(env.CODEX_HOME).not.toBe("private-profile");
    expect(env.HTTP_PROXY).toBe("http://127.0.0.1:1");
    expect(env.RELAY_CODEX_FIXTURE_KEY).toStartWith("sk-relay-synthetic-");
  });

  test("requires two generations, one audited tool result, strict config and isolated paths", async () => {
    const calls = [];
    const result = await qualifyCodexWire({ cliPath: process.execPath, catalog, client: fakeClient(calls) });
    expect(result.ok).toBe(true);
    expect(result.requests).toHaveLength(2);
    expect(result.toolCalls).toBe(1);
    const run = calls.find(call => call.args.includes("--strict-config"));
    expect(run.args).toContain("--ephemeral");
    expect(run.args).toContain("--ignore-user-config");
    expect(run.args).toContain("read-only");
    expect(run.args.some(value => value.includes("dangerously"))).toBe(false);
    expect(run.env.CODEX_HOME.startsWith(run.cwd)).toBe(true);
    await expect(stat(run.cwd)).rejects.toThrow();
  });

  for (const mode of [{ noCycle: true }, { badResult: true }, { fallbackCatalog: true }, { malformedFirst: true }]) {
    test(`a successful final message cannot hide a failed tool cycle: ${JSON.stringify(mode)}`, async () => {
      const result = await qualifyCodexWire({ cliPath: process.execPath, catalog, client: fakeClient([], mode) });
      expect(result.completion.ok).toBe(true);
      expect(result.ok).toBe(false);
    });
  }

  test("does not expose child keys, fixture capabilities or raw profile paths", async () => {
    const calls = [];
    const result = await qualifyCodexWire({ cliPath: process.execPath, catalog, client: fakeClient(calls, { diagnostics: true }) });
    const run = calls.find(call => call.args.includes("--strict-config"));
    expect(result.ok).toBe(false);
    expect(result.stderr).not.toContain(run.env.RELAY_CODEX_FIXTURE_KEY);
    expect(result.stderr).not.toContain(run.cwd);
    expect(result.stderr).not.toMatch(/[a-f0-9]{64}/);
    expect(result.stderr).toContain("<temporary-profile>");
  });

  test("an outbound proxy attempt fails qualification even after a complete cycle", async () => {
    const result = await qualifyCodexWire({ cliPath: process.execPath, catalog, client: fakeClient([], { outbound: true }) });
    expect(result.completion.ok).toBe(true);
    expect(result.generationRequests).toBe(2);
    expect(result.unexpectedMethods.CONNECT).toBe(1);
    expect(result.blockedDestinations.other).toBe(1);
    expect(result.ok).toBe(false);
  });

  test("retains multi-part client tool output in a content-and-order digest", async () => {
    const plain = await qualifyCodexWire({ cliPath: process.execPath, catalog, client: fakeClient([]) });
    const extended = await qualifyCodexWire({ cliPath: process.execPath, catalog, client: fakeClient([], { metadataPart: true }) });
    expect(plain.ok).toBe(true);
    expect(extended.ok).toBe(true);
    expect(extended.toolResultDigest).toMatch(/^[a-f0-9]{64}$/);
    expect(extended.toolResultDigest).not.toBe(plain.toolResultDigest);
  });

  test("rejects unscoped gateways and undeclared effort before starting the client", async () => {
    const client = async () => { throw new Error("client must not start"); };
    await expect(qualifyCodexWire({ cliPath: process.execPath, catalog, client, gatewayOrigin: "https://provider.example" })).rejects.toThrow("loopback");
    await expect(qualifyCodexWire({ cliPath: process.execPath, catalog, client, effort: "high" })).rejects.toThrow("declare");
  });

  test("a declared effort is retained verbatim rather than limited by a second hardcoded list", async () => {
    const selected = { models: [{ ...catalog.models[0], supported_reasoning_levels: [{ effort: "ultra" }] }] };
    const result = await qualifyCodexWire({ cliPath: process.execPath, catalog: selected, effort: "ultra", client: fakeClient([]) });
    expect(result.ok).toBe(true);
    expect(result.requests.every(request => request.effort === "ultra")).toBe(true);
  });

  test("real-provider mode requires explicit consent before starting the client", async () => {
    const client = async () => { throw new Error("client must not start"); };
    await expect(qualifyCodexWire({ cliPath: process.execPath, catalog, client,
      gatewayOrigin: "http://127.0.0.1:1", live: { key: "synthetic-private-gateway-key" } })).rejects.toThrow("consent");
  });

  test("live mode preserves dynamic call identity and keeps gateway credentials out of the child", async () => {
    const calls = [], received = [];
    const key = "synthetic-private-gateway-key";
    const gateway = createServer(async (request, response) => {
      const chunks = [];
      for await (const chunk of request) chunks.push(chunk);
      received.push({ authorization: request.headers.authorization, body: JSON.parse(Buffer.concat(chunks).toString("utf8")) });
      const output = received.length === 1
        ? [{ type: "function_call", call_id: "observed-call", name: FIXTURE_TOOL, arguments: '{"value": "synthetic"}' }]
        : [{ type: "message", role: "assistant", content: [{ type: "output_text", text: FIXTURE_RESULT }] }];
      response.writeHead(200, { "content-type": "text/event-stream" });
      response.end(`data: ${JSON.stringify({ type: "response.completed", response: { id: "observed-response", status: "completed", output } })}\n\n`);
    });
    await new Promise(resolve => gateway.listen(0, "127.0.0.1", resolve));
    try {
      const result = await qualifyCodexWire({ cliPath: process.execPath, catalog,
        client: fakeClient(calls, { liveAcknowledgement: true }),
        gatewayOrigin: `http://127.0.0.1:${gateway.address().port}`, live: { allowProviderRequests: true, key } });
      expect(result.ok).toBe(true);
      expect(result.mode).toBe("live_gateway");
      expect(result.resultEvidence.argumentsMatch).toBe(true);
      expect(result.responses).toHaveLength(2);
      expect(received.every(item => item.authorization === `Bearer ${key}`)).toBe(true);
      expect(received[1].body.input[1].call_id).toBe("observed-call");
      expect(JSON.stringify(calls)).not.toContain(key);
      expect(JSON.stringify(result)).not.toContain(key);
    } finally {
      gateway.closeAllConnections();
      await new Promise(resolve => gateway.close(resolve));
    }
  });
});
