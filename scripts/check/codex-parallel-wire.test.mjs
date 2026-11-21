import { expect, test } from "bun:test";
import { createServer } from "node:http";
import { qualifyCodexWire } from "./check-codex-wire.mjs";
import { PARALLEL_FIXTURE_RESULTS, PARALLEL_FIXTURE_RESULT } from "./fixtures/claude-tool-server.mjs";

const catalog = { models: [{ slug: "synthetic-model", base_instructions: "Synthetic instructions",
  supported_reasoning_levels: [{ effort: "high" }] }] };
const key = "synthetic-parallel-upstream-key";
const child = (stdout = "") => ({ code: 0, stdout, stderr: "", failure: null });
const completed = () => child([
  { type: "item.completed", item: { type: "agent_message", text: PARALLEL_FIXTURE_RESULT } },
  { type: "turn.completed" },
].map(value => JSON.stringify(value)).join("\n"));

async function runCase({ swapped = false, duplicateResult = false, duplicateCall = false,
  serialCall = false, forbiddenParallel = false, duplicateAudit = false, extraItem = false, extraFinalCall = false } = {}) {
  let sends = 0;
  const gateway = createServer(async (request, response) => {
    request.resume();
    expect(request.headers.authorization).toBe(`Bearer ${key}`);
    const output = ++sends === 1
      ? ["lookup_right", "lookup_left"].slice(0, serialCall ? 1 : 2).map((name, index) => ({ type: "function_call", name,
        namespace: "mcp__relay_contract", call_id: duplicateCall ? "same-call" : `observed-call-${index}`, arguments: '{"value": "synthetic"}' }))
      : [{ type: "message", role: "assistant", content: [{ type: "output_text", text: PARALLEL_FIXTURE_RESULT }] }];
    if ((extraItem && sends === 1) || (extraFinalCall && sends === 2)) {
      output.push({ type: "custom_tool_call", call_id: "unrelated-call", name: "unrelated", input: "synthetic" });
    }
    response.writeHead(200, { "content-type": "text/event-stream" });
    response.end(`data: ${JSON.stringify({ type: "response.completed", response: { id: "observed-response", status: "completed", output } })}\n\n`);
  });
  await new Promise(resolve => gateway.listen(0, "127.0.0.1", resolve));
  const client = async input => {
    if (input.args[0] === "--version") return child("codex-cli 0.1-fixture");
    if (input.args.includes("--help")) return child("--strict-config --ignore-user-config --ignore-rules --ephemeral --sandbox --json --skip-git-repo-check");
    expect(JSON.stringify(input)).not.toContain(key);
    const token = input.args.join(" ").match(/RELAY_TOOL_FIXTURE_TOKEN="([a-f0-9]{64})"/)[1];
    const origin = input.env.HTTP_PROXY;
    const headers = { authorization: `Bearer ${input.env.RELAY_CODEX_FIXTURE_KEY}`, "content-type": "application/json" };
    const body = { model: "synthetic-model", stream: true, reasoning: { effort: "high" }, parallel_tool_calls: !forbiddenParallel,
      instructions: catalog.models[0].base_instructions, input: [], tools: [{ type: "namespace", name: "mcp__relay_contract",
        tools: Object.keys(PARALLEL_FIXTURE_RESULTS).map(name => ({ type: "function", name })) }] };
    try {
      const first = await fetch(`${origin}/v1/responses`, { method: "POST", headers, body: JSON.stringify(body) });
      const raw = await first.text();
      if (first.status !== 200) return completed();
      const output = JSON.parse(raw.split("\n").find(line => line.startsWith("data:")).slice(5)).response.output;
      for (const call of output) {
        const audit = () => fetch(`${origin}/fixture/tool-call`, { method: "POST", headers: {
          "x-relay-fixture-token": token, "x-relay-fixture-tool": call.name } });
        await audit();
        if (duplicateAudit) await audit();
      }
      const results = output.map((call, index) => ({ type: "function_call_output", call_id: call.call_id,
        output: [{ type: "input_text", text: PARALLEL_FIXTURE_RESULTS[swapped ? output[1 - index].name : call.name] },
          { type: "input_text", text: "Synthetic client execution metadata" }] }));
      if (duplicateResult) results.push(results[0]);
      body.input = [...output, ...results.reverse()];
      const second = await fetch(`${origin}/v1/responses`, { method: "POST", headers, body: JSON.stringify(body) });
      await second.text();
    } catch { /* A rejected stream must not be hidden by a successful client exit. */ }
    return completed();
  };
  try {
    const result = await qualifyCodexWire({ cliPath: process.execPath, catalog, effort: "high", client,
      gatewayOrigin: `http://127.0.0.1:${gateway.address().port}`, scenario: "parallel", live: { allowProviderRequests: true, key } });
    return { result, sends };
  } finally {
    gateway.closeAllConnections();
    await new Promise(resolve => gateway.close(resolve));
  }
}

test("parallel qualification preserves namespace identities and independently returned result order", async () => {
  const { result, sends } = await runCase();
  expect(result.ok).toBe(true);
  expect(result.scenario).toBe("parallel");
  expect(result.toolCalls).toBe(2);
  expect(result.toolResultDigests).toHaveLength(2);
  expect(new Set(result.toolResultDigests).size).toBe(2);
  expect(result.resultEvidence.every(value => value.nameMatches && value.namespaceMatches && value.argumentsMatch && value.fixtureTextPresent)).toBe(true);
  expect(sends).toBe(2);
});

for (const mode of [{ swapped: true }, { duplicateResult: true }, { duplicateCall: true },
  { serialCall: true }, { forbiddenParallel: true }, { duplicateAudit: true }, { extraItem: true }]) {
  test(`parallel qualification rejects an invalid cycle without another provider generation: ${JSON.stringify(mode)}`, async () => {
    const { result, sends } = await runCase(mode);
    expect(result.completion.ok).toBe(true);
    expect(result.ok).toBe(false);
    expect(sends).toBeLessThanOrEqual(1);
  });
}

test("an unrelated final tool call cannot be hidden by the expected text or a successful client exit", async () => {
  const { result, sends } = await runCase({ extraFinalCall: true });
  expect(result.completion.ok).toBe(true);
  expect(result.ok).toBe(false);
  expect(sends).toBe(2);
});
