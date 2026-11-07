import { request as httpRequest } from "node:http";
import { StringDecoder } from "node:string_decoder";

export const FIXTURE_TOOL = "mcp__relay_contract__lookup";
export const FIXTURE_RESULT = "relay-tool-result-ok";
const VERSION = "2025-11-25";
const LINE_LIMIT = 64 * 1024;

/** A single deterministic tool, not a general filesystem or command server. */
export function createFixtureProtocol(onCall = async () => {}) {
  let initialized = false, ready = false, calls = 0;
  return async (message) => {
    const id = message?.id ?? null;
    const error = (code, text) => ({ jsonrpc: "2.0", id, error: { code, message: text } });
    const result = (value) => ({ jsonrpc: "2.0", id, result: value });
    if (!message || Array.isArray(message) || message.jsonrpc !== "2.0" || typeof message.method !== "string") return error(-32600, "Invalid fixture request");
    if (message.id === undefined) {
      if (message.method === "notifications/initialized" && initialized) ready = true;
      return null;
    }
    if (!(Number.isSafeInteger(id) || (typeof id === "string" && id.length <= 128))) return error(-32600, "Invalid fixture request ID");
    if (message.method === "ping") return result({});
    if (message.method === "initialize") {
      if (initialized || typeof message.params?.protocolVersion !== "string") return error(-32602, "Invalid fixture initialization");
      initialized = true;
      return result({ protocolVersion: VERSION, capabilities: { tools: { listChanged: false } }, serverInfo: { name: "relay-contract-fixture", version: "1.0.0" } });
    }
    if (!ready) return error(-32600, "Fixture initialization is incomplete");
    if (message.method === "tools/list") return result({ tools: [{
      name: "lookup", description: "Return the deterministic synthetic fixture result.",
      inputSchema: { type: "object", properties: { value: { type: "string", enum: ["synthetic"] } }, required: ["value"], additionalProperties: false },
      annotations: { readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
    }] });
    if (message.method !== "tools/call") return error(-32601, "Unknown fixture method");
    const params = message.params;
    if (params?.name !== "lookup" || !params.arguments || Array.isArray(params.arguments)
      || Object.keys(params.arguments).length !== 1 || params.arguments.value !== "synthetic") return error(-32602, "Invalid fixture tool arguments");
    if (++calls !== 1) return result({ content: [{ type: "text", text: "Fixture tool was called more than once" }], isError: true });
    try {
      await onCall();
      return result({ content: [{ type: "text", text: FIXTURE_RESULT }], isError: false });
    } catch {
      return result({ content: [{ type: "text", text: "Fixture audit callback failed" }], isError: true });
    }
  };
}

function auditCallback() {
  const target = new URL(process.env.RELAY_TOOL_FIXTURE_ORIGIN);
  const token = process.env.RELAY_TOOL_FIXTURE_TOKEN;
  if (target.protocol !== "http:" || target.hostname !== "127.0.0.1" || !target.port
    || target.pathname !== "/" || target.username || target.password || target.search || target.hash
    || !/^[a-f0-9]{64}$/.test(token ?? "")) throw new Error("Invalid loopback fixture audit configuration");
  return () => new Promise((resolve, reject) => {
    const request = httpRequest(new URL("/fixture/tool-call", target), {
      method: "POST", headers: { "x-relay-fixture-token": token }, signal: AbortSignal.timeout(5000),
    }, (response) => {
      response.resume();
      response.once("end", () => response.statusCode === 204 ? resolve() : reject(new Error("Fixture audit rejected")));
      response.once("error", reject);
    });
    request.once("error", reject);
    request.end();
  });
}

async function serveStdio() {
  const handle = createFixtureProtocol(auditCallback());
  const decoder = new StringDecoder("utf8");
  let pending = "", count = 0;
  const timeout = setTimeout(() => process.exit(2), 30000);
  try {
    for await (const chunk of process.stdin) {
      pending += decoder.write(chunk);
      let index;
      while ((index = pending.indexOf("\n")) !== -1) {
        const line = pending.slice(0, index);
        pending = pending.slice(index + 1);
        if (Buffer.byteLength(line) > LINE_LIMIT || ++count > 64) throw new Error("Fixture input limit");
        let message;
        try { message = JSON.parse(line); } catch { throw new Error("Invalid fixture JSON"); }
        const reply = await handle(message);
        if (reply) process.stdout.write(`${JSON.stringify(reply)}\n`);
      }
      if (Buffer.byteLength(pending) > LINE_LIMIT) throw new Error("Fixture line limit");
    }
    if (pending.length || decoder.end().length) throw new Error("Truncated fixture input");
  } finally { clearTimeout(timeout); }
}

if (import.meta.main) {
  try { await serveStdio(); }
  catch { console.error("Synthetic MCP fixture stopped"); process.exitCode = 2; }
}
