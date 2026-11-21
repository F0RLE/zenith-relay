import { expect, test } from "bun:test";
import { createServer } from "node:http";
import { forwardCodexGateway } from "./fixtures/codex-gateway-forward.mjs";
import { CodexQualificationOutput } from "./fixtures/codex-qualification-output.mjs";

const listen = async server => {
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  return `http://127.0.0.1:${server.address().port}`;
};
const close = async server => {
  server.closeAllConnections();
  await new Promise(resolve => server.close(resolve));
};

test("gateway observer forwards bytes before EOF and preserves multiline UTF-8 SSE", async () => {
  let release;
  const gate = new Promise(resolve => { release = resolve; });
  const first = ': synthetic comment\r\nevent: response.output_text.delta\r\ndata: {"type":"response.output_text.delta",\r\ndata: "delta":"hello🙂"}\r\n\r\n';
  const last = `data: ${JSON.stringify({ type: "response.completed", response: { status: "completed", usage: { input_tokens: 12, output_tokens: 2 } } })}\n\n`;
  const upstream = createServer(async (request, response) => {
    request.resume();
    response.writeHead(200, { "content-type": "text/event-stream" });
    response.write(Buffer.from(first).subarray(0, Buffer.byteLength(first) - 5));
    response.write(Buffer.from(first).subarray(Buffer.byteLength(first) - 5));
    await gate;
    response.end(last);
  });
  const origin = await listen(upstream);
  let observed, error;
  const proxy = createServer((_request, response) => {
    forwardCodexGateway({ origin, bytes: Buffer.from("{}"), response, timeoutMs: 3000, key: "synthetic" })
      .then(value => { observed = value; }, value => { error = value; });
  });
  const proxyOrigin = await listen(proxy);
  try {
    const response = await fetch(proxyOrigin, { signal: AbortSignal.timeout(2500) });
    const reader = response.body.getReader();
    const parts = [];
    let received = 0;
    while (received < Buffer.byteLength(first)) {
      const chunk = await reader.read();
      expect(chunk.done).toBe(false);
      parts.push(chunk.value);
      received += chunk.value.length;
    }
    expect(Buffer.concat(parts).toString("utf8")).toContain("hello🙂");
    expect(observed).toBeUndefined();
    release();
    for (;;) {
      const part = await reader.read();
      if (part.done) break;
      parts.push(part.value);
    }
    await new Promise(resolve => setImmediate(resolve));
    expect(error).toBeUndefined();
    expect(Buffer.concat(parts).toString("utf8")).toBe(first + last);
    expect(observed.terminalType).toBe("response.completed");
    expect(observed.terminalStatus).toBe("completed");
    expect(observed.eventTypes["response.output_text.delta"]).toBe(1);
    expect(observed.firstTextMs).toBeGreaterThanOrEqual(0);
    expect(observed.usage.cachedInputTokens).toBeNull();
    expect(observed.bytes).toBe(Buffer.byteLength(first + last));
  } finally {
    release();
    await close(proxy);
    await close(upstream);
  }
});

test("gateway observer rejects a non-loopback origin before touching a response", async () => {
  await expect(forwardCodexGateway({ origin: "https://provider.invalid", response: null })).rejects.toThrow("loopback");
});

test("a sparse terminal qualifies indexed completed items in output order", () => {
  const proof = new CodexQualificationOutput();
  const reasoning = { type: "reasoning", id: "rs_test", summary: [] };
  const call = { type: "function_call", id: "fc_test", call_id: "call_test", name: "lookup", arguments: "{}" };
  const event = (suffix, output_index, item) => ({ type: `response.output_item.${suffix}`, output_index, item });
  proof.observe(event("added", 0, reasoning));
  proof.observe(event("done", 1, call));
  proof.observe(event("done", 0, reasoning));
  const terminal = { id: "resp_test", status: "completed", output: [] };
  expect(proof.finish(terminal).output).toEqual([reasoning, call]);
  expect(terminal.output).toEqual([]);
  expect(proof.finish({ ...terminal, output: [reasoning, call] }).output).toEqual([reasoning, call]);
});

test("a sparse terminal cannot hide a pending, unindexed or contradictory output", () => {
  const item = { type: "function_call", id: "fc_test", arguments: "{}" };
  const done = { type: "response.output_item.done", output_index: 0, item };
  const terminal = { id: "resp_test", status: "completed", output: [] };
  for (const events of [
    [{ ...done, output_index: 1 }], [{ ...done, output_index: undefined }],
    [{ ...done, type: "response.output_item.added" }],
    [{ type: "response.output_text.delta", output_index: 1, delta: "synthetic" }, done],
    [{ type: "response.output_text.delta", delta: "synthetic" }, done],
    [done, { type: "response.function_call_arguments.delta", output_index: 0, delta: "extra" }],
    [{ ...done, response_id: "first" }, { type: "response.completed", response: { id: "second" } }],
    [done, { ...done, item: { ...item, arguments: "changed" } }],
    [{ ...done, type: "response.output_item.added" }, { ...done, item: { ...item, id: "another" } }],
    [{ ...done, item: null }], [{ ...done, output_index: 2 ** 40 }],
  ]) {
    const proof = new CodexQualificationOutput();
    expect(() => { for (const e of events) proof.observe(e); proof.finish(terminal); }).toThrow();
  }
  const proof = new CodexQualificationOutput();
  proof.observe(done);
  expect(() => proof.finish({ ...terminal, output: [{ ...item, arguments: "different" }] })).toThrow();
});
