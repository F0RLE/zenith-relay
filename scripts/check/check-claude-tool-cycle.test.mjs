import { describe, expect, test } from "bun:test";
import { isAbsolute } from "node:path";
import { qualifyEffortWire } from "./check-claude-effort-wire.mjs";
import { createFixtureProtocol, FIXTURE_TOOL, FIXTURE_RESULT } from "./fixtures/claude-tool-server.mjs";

const model = { name: "claude-relay-0123-4567-89ab-cdef", behavesAs: "claude-opus-4-7", maxEffort: "max" };
const valueOf = (args, flag) => args[args.indexOf(flag) + 1];
const help = "--bare --tools --settings --setting-sources --strict-mcp-config --mcp-config --effort --no-session-persistence --output-format --system-prompt --disable-slash-commands --allowedTools";

function simulatedClient({ audit = true, invalidToken = false, duplicateAudit = false, declared = true, reuseToken = false, mutateResult = result => result, mutateHistory = messages => messages } = {}) {
  let firstToken = null;
  return async (input) => {
    if (input.args[0] === "--version") return { code: 0, stdout: "2.1.309 (Claude Code)\n", stderr: "" };
    if (input.args[0] === "--help") return { code: 0, stdout: help, stderr: "" };
    expect(valueOf(input.args, "--max-turns")).toBe("2");
    expect(valueOf(input.args, "--tools")).toBe("");
    expect(valueOf(input.args, "--allowedTools")).toBe(FIXTURE_TOOL);
    expect(valueOf(input.args, "--setting-sources")).toBe("");
    expect(input.args.some(arg => arg.includes("dangerously"))).toBeFalse();
    const mcp = JSON.parse(valueOf(input.args, "--mcp-config"));
    expect(Object.keys(mcp.mcpServers)).toEqual(["relay_contract"]);
    const server = mcp.mcpServers.relay_contract;
    expect(server.type).toBe("stdio");
    expect(server.command).toBe(process.execPath);
    expect(server.args).toHaveLength(1);
    expect(isAbsolute(server.args[0])).toBeTrue();
    expect(server.args[0].endsWith("claude-tool-server.mjs")).toBeTrue();
    expect(server.env.RELAY_TOOL_FIXTURE_ORIGIN).toBe(input.env.ANTHROPIC_BASE_URL);
    firstToken ??= server.env.RELAY_TOOL_FIXTURE_TOKEN;
    const callAudit = async () => {
      if (!audit) return;
      for (let index = 0; index < (duplicateAudit ? 2 : 1); index++) {
        const response = await fetch(`${server.env.RELAY_TOOL_FIXTURE_ORIGIN}/fixture/tool-call`, {
          method: "POST", headers: { "x-relay-fixture-token": invalidToken ? "invalid" : reuseToken ? firstToken : server.env.RELAY_TOOL_FIXTURE_TOKEN },
        });
        await response.arrayBuffer();
        if (response.status !== 204) throw new Error("Synthetic audit rejected");
      }
    };
    const handle = createFixtureProtocol(callAudit);
    await handle({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2025-11-25" } });
    await handle({ jsonrpc: "2.0", method: "notifications/initialized" });
    const request = async (body) => {
      const response = await fetch(`${input.env.ANTHROPIC_BASE_URL}/v1/messages`, {
        method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body),
      });
      expect(response.status).toBe(200);
      expect(await response.text()).toContain("event: message_stop");
    };
    const body = {
      model: valueOf(input.args, "--model"), stream: true,
      output_config: { effort: valueOf(input.args, "--effort") },
      tools: declared ? [{ name: FIXTURE_TOOL, input_schema: { type: "object" } }] : [],
      messages: [{ role: "user", content: "Synthetic tool-cycle request." }],
    };
    await request(body);
    const tool = await handle({ jsonrpc: "2.0", id: 2, method: "tools/call", params: { name: "lookup", arguments: { value: "synthetic" } } });
    body.messages.push({ role: "assistant", content: [{ type: "tool_use", id: "toolu_relay_fixture", name: FIXTURE_TOOL, input: { value: "synthetic" } }] });
    body.messages.push({ role: "user", content: [mutateResult({ type: "tool_result", tool_use_id: "toolu_relay_fixture", content: tool.result.content, is_error: tool.result.isError })] });
    body.messages = mutateHistory(body.messages);
    await request(body);
    return { code: 0, stdout: JSON.stringify({ type: "result", subtype: "success", is_error: false, result: "relay-contract-ok" }), stderr: "" };
  };
}

describe("installed-client tool-cycle qualification", () => {
  const qualify = driver => qualifyEffortWire({ cliPath: process.execPath, model, efforts: ["max"], toolCycle: true, driver });

  test("requires one independently audited tool execution and the associated next-turn result", async () => {
    const result = await qualify(simulatedClient());
    expect(result.ok).toBeTrue();
    expect(result.toolCycle).toBeTrue();
    expect(result.cases[0]).toMatchObject({ messageRequests: 2, fixtureToolCalls: 1, declaredFixtureTool: true, toolResultReceived: true });
    expect(result.cases[0].requests.every(request => request.model === model.name && request.effort === "max")).toBeTrue();
    expect(JSON.stringify(result)).not.toContain("RELAY_TOOL_FIXTURE_TOKEN");
  });

  test("does not accept a fabricated successful client result without an audited execution", async () => {
    expect((await qualify(simulatedClient({ audit: false }))).ok).toBeFalse();
  });

  test("rejects changed result identities, content and error results", async () => {
    for (const mutateResult of [
      result => ({ ...result, tool_use_id: "toolu_wrong" }),
      result => ({ ...result, content: [{ type: "text", text: "wrong result" }] }),
      result => ({ ...result, content: [{ type: "text", text: FIXTURE_RESULT }, { type: "text", text: "extra" }] }),
      result => ({ ...result, is_error: true }),
    ]) expect((await qualify(simulatedClient({ mutateResult }))).ok).toBeFalse();
  });

  test("rejects undeclared tools, duplicate callbacks and another case's audit token", async () => {
    for (const options of [{ declared: false }, { duplicateAudit: true }, { invalidToken: true }]) {
      expect((await qualify(simulatedClient(options))).ok).toBeFalse();
    }
  });

  test("rejects a result moved before the original tool call", async () => {
    expect((await qualify(simulatedClient({ mutateHistory: messages => [messages[0], messages[2], messages[1]] }))).ok).toBeFalse();
  });

  test("an earlier case cannot authorize a later tool execution", async () => {
    const result = await qualifyEffortWire({ cliPath: process.execPath, model, efforts: ["max", "xhigh"], toolCycle: true, driver: simulatedClient({ reuseToken: true }) });
    expect(result.cases[0].ok).toBeTrue();
    expect(result.cases[1].ok).toBeFalse();
    expect(result.ok).toBeFalse();
  });

  test("redacts the fixture audit capability from child-error diagnostics", async () => {
    const base = simulatedClient();
    let token;
    const result = await qualify(async input => {
      const child = await base(input);
      if (input.args[0] !== "--bare") return child;
      token = JSON.parse(valueOf(input.args, "--mcp-config")).mcpServers.relay_contract.env.RELAY_TOOL_FIXTURE_TOKEN;
      return { ...child, code: 1, stderr: `${input.env.ANTHROPIC_API_KEY} ${token}` };
    });
    expect(result.ok).toBeFalse();
    expect(result.cases[0].stderr).toBe("<synthetic-key> <fixture-token>");
    expect(JSON.stringify(result)).not.toContain(token);
  });
});
