import { spawn } from "node:child_process";
import { createServer, request as httpRequest } from "node:http";
import { mkdir, mkdtemp, realpath, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { isAbsolute, join, relative, sep } from "node:path";
import { parseArgs } from "node:util";
import { fileURLToPath } from "node:url";
import { randomBytes } from "node:crypto";
import { FIXTURE_TOOL, FIXTURE_RESULT } from "./fixtures/claude-tool-server.mjs";

const ACK = "relay-contract-ok";
const BODY_LIMIT = 512 * 1024;
const OUTPUT_LIMIT = 64 * 1024;
const EFFORTS = new Set(["low", "medium", "high", "xhigh", "max"]);
// Print-only bounds can be absent from public help. The actual run still
// requires --max-turns; the driver and fixture also enforce independent bounds.
const PUBLIC_REQUIRED_FLAGS = [
  "--bare", "--tools", "--settings", "--setting-sources", "--strict-mcp-config",
  "--mcp-config", "--effort", "--no-session-persistence", "--output-format",
  "--system-prompt", "--disable-slash-commands",
];

function validateModel(model) {
  if (!model || Array.isArray(model) || typeof model !== "object"
    || Object.keys(model).some((key) => !["name", "behavesAs", "labelOverride", "maxEffort"].includes(key))) {
    throw new Error("Supply one generated Claude model descriptor");
  }
  for (const key of ["name", "behavesAs"]) {
    if (typeof model[key] !== "string" || !/^claude-[A-Za-z0-9._-]{1,240}$/.test(model[key])) {
      throw new Error(`Invalid generated model ${key}`);
    }
  }
  if (model.maxEffort !== undefined && !EFFORTS.has(model.maxEffort)) throw new Error("Invalid generated model maxEffort");
}

export function isolatedEnvironment(root, origin, parent = process.env) {
  const env = {};
  for (const key of ["SystemRoot", "SYSTEMROOT", "WINDIR", "COMSPEC", "PATH", "PATHEXT", "SystemDrive"]) {
    if (parent[key]) env[key] = parent[key];
  }
  return {
    ...env,
    HOME: root, USERPROFILE: root, APPDATA: join(root, "roaming"),
    LOCALAPPDATA: join(root, "local"), TEMP: root, TMP: root,
    CLAUDE_CONFIG_DIR: join(root, "config"),
    ANTHROPIC_API_KEY: "sk-ant-synthetic-loopback-contract",
    ANTHROPIC_BASE_URL: origin,
    CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: "1",
    DISABLE_AUTOUPDATER: "1", DISABLE_TELEMETRY: "1", DISABLE_ERROR_REPORTING: "1",
    HTTP_PROXY: origin, HTTPS_PROXY: origin, ALL_PROXY: origin,
    NO_PROXY: "127.0.0.1", no_proxy: "127.0.0.1", TERM: "dumb",
  };
}

export function runClient({ cliPath, args, cwd, env, timeoutMs }) {
  return new Promise((resolve) => {
    const child = spawn(cliPath, args, { cwd, env, windowsHide: true, shell: false, stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "", stderr = "", bytes = 0, failure = null;
    const timer = setTimeout(() => { failure ??= "client_timeout"; child.kill(); }, timeoutMs);
    const collect = (field, chunk) => {
      bytes += chunk.length;
      if (bytes > OUTPUT_LIMIT) { failure ??= "client_output_limit"; child.kill(); return; }
      if (field === "stdout") stdout += chunk.toString("utf8");
      else stderr += chunk.toString("utf8");
    };
    child.stdout?.on("data", (chunk) => collect("stdout", chunk));
    child.stderr?.on("data", (chunk) => collect("stderr", chunk));
    child.once("error", () => { failure ??= "client_spawn_failed"; });
    child.once("close", (code) => {
      clearTimeout(timer);
      resolve({ code, stdout, stderr, failure });
    });
  });
}

function replyMessage(response, body, tool = false) {
  const message = {
    id: "msg_relay_contract", type: "message", role: "assistant", model: body.model,
    content: [tool ? { type: "tool_use", id: "toolu_relay_fixture", name: FIXTURE_TOOL, input: { value: "synthetic" } } : { type: "text", text: ACK }], stop_reason: tool ? "tool_use" : "end_turn", stop_sequence: null,
    usage: { input_tokens: 5, output_tokens: 5 },
  };
  if (!body.stream) {
    response.writeHead(200, { "content-type": "application/json" });
    response.end(JSON.stringify(message));
    return;
  }
  const events = [
    ["message_start", { type: "message_start", message: { ...message, content: [], stop_reason: null, usage: { input_tokens: 5, output_tokens: 0 } } }],
    ["content_block_start", { type: "content_block_start", index: 0, content_block: tool ? { type: "tool_use", id: "toolu_relay_fixture", name: FIXTURE_TOOL, input: {} } : { type: "text", text: "" } }],
    ["content_block_delta", { type: "content_block_delta", index: 0, delta: tool ? { type: "input_json_delta", partial_json: '{"value":"synthetic"}' } : { type: "text_delta", text: ACK } }],
    ["content_block_stop", { type: "content_block_stop", index: 0 }],
    ["message_delta", { type: "message_delta", delta: { stop_reason: message.stop_reason, stop_sequence: null }, usage: { output_tokens: 5 } }],
    ["message_stop", { type: "message_stop" }],
  ];
  response.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-store" });
  response.end(events.map(([event, value]) => `event: ${event}\ndata: ${JSON.stringify(value)}\n\n`).join(""));
}

function hasFixtureToolResult(body) {
  const messages = Array.isArray(body.messages) ? body.messages : [];
  const calls = [], results = [];
  for (const [index, message] of messages.entries()) {
    for (const block of Array.isArray(message.content) ? message.content : []) {
      if (message.role === "assistant" && block.type === "tool_use" && block.id === "toolu_relay_fixture") calls.push({ block, index });
      if (message.role === "user" && block.type === "tool_result" && block.tool_use_id === "toolu_relay_fixture") results.push({ block, index });
    }
  }
  if (calls.length !== 1 || results.length !== 1 || results[0].index <= calls[0].index) return false;
  const call = calls[0].block, result = results[0].block;
  if (call.name !== FIXTURE_TOOL || call.input?.value !== "synthetic"
    || Object.keys(call.input).length !== 1 || result.is_error === true) return false;
  const content = result.content;
  return content === FIXTURE_RESULT || (Array.isArray(content) && content.length === 1 && content[0].type === "text" && content[0].text === FIXTURE_RESULT);
}

function completionStatus(stdout) {
  try {
    const value = JSON.parse(stdout.trim());
    return { type: value.type, subtype: value.subtype, isError: value.is_error === true, ok: value.type === "result" && value.is_error !== true && value.result === ACK };
  } catch {
    return { ok: false };
  }
}

function loopbackGatewayOrigin(value) {
  if (value === undefined) return null;
  const url = new URL(value);
  if (url.protocol !== "http:" || url.hostname !== "127.0.0.1" || !url.port
    || url.username || url.password || url.pathname !== "/" || url.search || url.hash) {
    throw new Error("Gateway must be an explicit loopback HTTP origin");
  }
  return url.origin;
}

async function forwardToGateway(origin, path, request, bytes, response, timeoutMs) {
  const headers = { "content-type": "application/json", "x-api-key": "sk-ant-synthetic-loopback-contract" };
  for (const key of ["anthropic-version", "anthropic-beta"]) {
    if (typeof request.headers[key] === "string") headers[key] = request.headers[key];
  }
  const upstream = await new Promise((resolve, reject) => {
    const sent = httpRequest(new URL(path, origin), {
      method: "POST", headers, signal: AbortSignal.timeout(timeoutMs),
    }, resolve);
    sent.once("error", reject);
    sent.end(bytes);
  });
  let length = 0;
  const chunks = [];
  for await (const chunk of upstream) {
    length += chunk.length;
    if (length > BODY_LIMIT) { upstream.destroy(); throw new Error("Gateway fixture reply exceeds its limit"); }
    chunks.push(chunk);
  }
  const payload = Buffer.concat(chunks);
  const diagnostics = { status: upstream.statusCode, errorCodes: [], eventTypes: [] };
  const category = upstream.headers["x-zenith-relay-error-category"];
  if (typeof category === "string" && /^[a-z_]{1,80}$/.test(category)) diagnostics.category = category;
  const classify = (value) => {
    if (typeof value?.type === "string" && /^[a-z_.]{1,80}$/.test(value.type)) diagnostics.eventTypes.push(value.type);
    for (const code of [value?.error?.code, value?.error?.type]) {
      if (typeof code === "string" && /^[a-z_]{1,80}$/.test(code)) diagnostics.errorCodes.push(code);
    }
    const message = value?.error?.message;
    const parameter = value?.error?.param ?? (typeof message === "string" ? message.match(/\(parameter: ([A-Za-z0-9_.\[\]*-]{1,128})\)/)?.[1] : null);
    if (typeof parameter === "string" && /^[A-Za-z0-9_.\[\]*-]{1,128}$/.test(parameter)) diagnostics.parameter = parameter;
  };
  for (const text of upstream.headers["content-type"]?.includes("text/event-stream")
    ? payload.toString("utf8").split("\n").filter(line => line.startsWith("data:")).map(line => line.slice(5).trim())
    : [payload.toString("utf8")]) {
    try { classify(JSON.parse(text)); } catch { /* A malformed frame is not an inferred provider error. */ }
  }
  diagnostics.errorCodes = [...new Set(diagnostics.errorCodes)].slice(0, 16);
  diagnostics.eventTypes = [...new Set(diagnostics.eventTypes)].slice(0, 32);
  response.writeHead(upstream.statusCode, { "content-type": upstream.headers["content-type"] ?? "application/json" });
  response.end(payload);
  return diagnostics;
}

export async function qualifyEffortWire({ cliPath, model, efforts = ["max", "xhigh"], timeoutMs = 20000, withNativeControl = false, gatewayOrigin, toolCycle = false, driver = runClient }) {
  validateModel(model);
  const gateway = loopbackGatewayOrigin(gatewayOrigin);
  if (gateway && withNativeControl) throw new Error("Gateway qualification requires the generated alias route");
  if (toolCycle && withNativeControl) throw new Error("Tool-cycle qualification requires the generated alias route");
  if (!isAbsolute(cliPath) || !(await stat(cliPath)).isFile()) throw new Error("Select an existing absolute Claude Code executable path");
  if (!Array.isArray(efforts) || !efforts.length || efforts.some((effort) => !EFFORTS.has(effort)) || new Set(efforts).size !== efforts.length) throw new Error("Supply distinct native effort levels");
  if (model.maxEffort && efforts.some((effort) => [...EFFORTS].indexOf(effort) > [...EFFORTS].indexOf(model.maxEffort))) throw new Error("Requested effort exceeds the generated model cap");
  if (!Number.isInteger(timeoutMs) || timeoutMs < 1000 || timeoutMs > 60000) throw new Error("Timeout must be between 1000 and 60000 ms");
  const root = await realpath(await mkdtemp(join(tmpdir(), "relay-claude-wire-")));
  const server = createServer();
  let active = null, activeToken = null, blockedOutbound = 0, unscopedRequests = 0;
  try {
    await Promise.all(["config", "roaming", "local"].map((dir) => mkdir(join(root, dir))));
    await new Promise((resolve, reject) => {
      server.once("error", reject);
      server.listen(0, "127.0.0.1", resolve);
    });
    const origin = `http://127.0.0.1:${server.address().port}`;
    server.on("connect", (_request, socket) => { blockedOutbound++; socket.end("HTTP/1.1 403 Forbidden\r\nConnection: close\r\n\r\n"); });
    server.on("request", async (request, response) => {
      const scope = active;
      try {
        const url = new URL(request.url, origin);
        if (url.origin !== origin) { blockedOutbound++; response.writeHead(403).end(); return; }
        if (toolCycle && request.method === "POST" && url.pathname === "/fixture/tool-call") {
          request.resume();
          if (!scope) { unscopedRequests++; response.writeHead(403).end(); return; }
          if (request.headers["x-relay-fixture-token"] !== activeToken || scope.messageRequests !== 1 || scope.fixtureToolCalls !== 0) {
            scope.unexpectedRequests++; response.writeHead(403).end(); return;
          }
          scope.fixtureToolCalls++;
          response.writeHead(204).end(); return;
        }
        if (request.method !== "POST" || !["/v1/messages", "/v1/messages/count_tokens"].includes(url.pathname)) {
          if (scope) scope.unexpectedRequests++;
          else unscopedRequests++;
          response.writeHead(404).end(); return;
        }
        if (!scope) { unscopedRequests++; response.writeHead(429).end(); return; }
        const tokenCount = url.pathname.endsWith("/count_tokens");
        if (tokenCount) scope.tokenCountRequests++;
        else if (++scope.messageRequests > 4) { response.writeHead(429).end(); return; }
        let length = 0;
        const chunks = [];
        for await (const chunk of request) {
          length += chunk.length;
          if (length > BODY_LIMIT) { response.writeHead(413).end(); return; }
          chunks.push(chunk);
        }
        const bytes = Buffer.concat(chunks);
        const body = JSON.parse(bytes.toString("utf8"));
        if (tokenCount) {
          response.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({ input_tokens: 5 })); return;
        }
        const observed = { model: body.model, effort: body.output_config?.effort ?? null, streaming: body.stream === true };
        if (toolCycle && Object.hasOwn(body, "context_management")) {
          const context = body.context_management;
          observed.contextManagement = {
            shape: context === null ? "null" : Array.isArray(context) ? "array" : typeof context,
            fields: context && typeof context === "object" ? Object.keys(context).filter(key => ["edits"].includes(key)) : [],
            editCount: Array.isArray(context?.edits) ? context.edits.length : null,
            editTypes: Array.isArray(context?.edits) ? context.edits.map(edit => ["clear_tool_uses_20250919", "clear_thinking_20251015", "compact_20260112"].includes(edit?.type) ? edit.type : "other").slice(0, 8) : [],
            keeps: Array.isArray(context?.edits) ? context.edits.map(edit => edit?.keep === "all" ? "all" : edit?.keep == null ? "unset" : "other").slice(0, 8) : [],
          };
        }
        scope.requests.push(observed);
        if (toolCycle) {
          observed.thinkingDisplay = ["summarized", "omitted"].includes(body.thinking?.display) ? body.thinking.display : body.thinking?.display === undefined ? "unset" : "other";
          if (scope.messageRequests === 1) scope.declaredFixtureTool = Array.isArray(body.tools) && body.tools.some(tool => tool.name === FIXTURE_TOOL);
          else scope.toolResultReceived = hasFixtureToolResult(body);
        }
        if (gateway) observed.gateway = await forwardToGateway(gateway, `${url.pathname}${url.search}`, request, bytes, response, timeoutMs);
        else replyMessage(response, body, toolCycle && scope.messageRequests === 1);
      } catch {
        if (scope) scope.invalidRequests++;
        else unscopedRequests++;
        if (!response.headersSent) response.writeHead(400);
        response.end();
      }
    });
    const env = isolatedEnvironment(root, origin);
    const version = await driver({ cliPath, args: ["--version"], cwd: root, env, timeoutMs });
    if (version.code !== 0 || !/^\d+\.\d+\.\d+.*\(Claude Code\)\s*$/.test(version.stdout)) throw new Error("The selected executable did not identify itself as Claude Code");
    const help = await driver({ cliPath, args: ["--help"], cwd: root, env, timeoutMs });
    const missingFlags = [...PUBLIC_REQUIRED_FLAGS, ...(toolCycle ? ["--allowedTools"] : [])].filter((flag) => !help.stdout.includes(flag));
    if (help.code !== 0 || missingFlags.length) throw new Error(`The selected Claude Code lacks the required isolation or effort flags: ${missingFlags.join(", ")}`);
    const cases = [];
    for (const route of withNativeControl ? ["native_control", "generated_alias"] : ["generated_alias"]) {
      for (const effort of efforts) {
        const requestedModel = route === "generated_alias" ? model.name : model.behavesAs;
        const settings = route === "generated_alias" ? { modelPicker: { options: [{ model: model.name, behavesAs: model.behavesAs }] } } : {};
        active = { requests: [], messageRequests: 0, tokenCountRequests: 0, unexpectedRequests: 0, invalidRequests: 0,
          ...(toolCycle ? { fixtureToolCalls: 0, declaredFixtureTool: false, toolResultReceived: false } : {}) };
        activeToken = randomBytes(32).toString("hex");
        const mcp = toolCycle ? { mcpServers: { relay_contract: {
          type: "stdio", command: process.execPath,
          args: [fileURLToPath(new URL("./fixtures/claude-tool-server.mjs", import.meta.url))],
          env: { RELAY_TOOL_FIXTURE_ORIGIN: origin, RELAY_TOOL_FIXTURE_TOKEN: activeToken },
        } } } : { mcpServers: {} };
        const child = await driver({
          cliPath, cwd: root, env, timeoutMs,
          args: ["--bare", "--print", "--no-session-persistence", "--output-format", "json", "--max-turns", toolCycle ? "2" : "1", "--tools", "", "--disable-slash-commands", "--strict-mcp-config", "--mcp-config", JSON.stringify(mcp), ...(toolCycle ? ["--allowedTools", FIXTURE_TOOL] : []), "--setting-sources", "", "--settings", JSON.stringify(settings), "--model", requestedModel, "--effort", effort, "--system-prompt", "Synthetic loopback wire qualification.", toolCycle ? "Call the supplied synthetic lookup tool, then return the gateway acknowledgement." : "Return the gateway acknowledgement."],
        });
        const wire = active;
        const token = activeToken;
        active = null;
        activeToken = null;
        const completion = completionStatus(child.stdout);
        cases.push({ route, requestedEffort: effort, expectedModel: requestedModel, ...wire, exitCode: child.code, completion, failure: child.failure ?? null,
          stderr: child.code === 0 ? "" : child.stderr.replaceAll(env.ANTHROPIC_API_KEY, "<synthetic-key>").replaceAll(token, "<fixture-token>").slice(0, 2000),
          ok: child.code === 0 && !child.failure && completion.ok && wire.messageRequests === (toolCycle ? 2 : 1) && wire.requests.length === (toolCycle ? 2 : 1) && wire.unexpectedRequests === 0 && wire.invalidRequests === 0
            && wire.requests.every(request => request.model === requestedModel && request.effort === effort)
            && (!toolCycle || (wire.fixtureToolCalls === 1 && wire.declaredFixtureTool && wire.toolResultReceived)),
        });
      }
    }
    return { clientVersion: version.stdout.trim(), mode: gateway ? "relay_gateway" : "direct_fixture", toolCycle, cases, blockedOutbound, unscopedRequests, ok: blockedOutbound === 0 && unscopedRequests === 0 && cases.every((row) => row.ok) };
  } finally {
    server.closeAllConnections?.();
    if (server.listening) await new Promise((resolve) => server.close(resolve));
    const resolved = await realpath(root);
    const location = relative(await realpath(tmpdir()), resolved);
    if (resolved !== root || isAbsolute(location) || location.startsWith(`..${sep}`) || location === ".." || !location.startsWith("relay-claude-wire-")) throw new Error("Temporary probe cleanup containment failed");
    await rm(resolved, { recursive: true });
  }
}

if (import.meta.main) {
  try {
    const { values } = parseArgs({ options: { cli: { type: "string" }, model: { type: "string" }, efforts: { type: "string", default: "max,xhigh" }, timeout: { type: "string", default: "20000" }, gateway: { type: "string" }, "tool-cycle": { type: "boolean", default: false }, "with-native-control": { type: "boolean", default: false } } });
    if (!values.cli || !values.model) throw new Error("Usage: bun scripts/check/check-claude-effort-wire.mjs --cli ABSOLUTE_PATH --model GENERATED_MODEL_JSON [--efforts max,xhigh] [--with-native-control | --gateway HTTP_LOOPBACK_ORIGIN]");
    const result = await qualifyEffortWire({ cliPath: values.cli, model: JSON.parse(values.model), efforts: values.efforts.split(","), timeoutMs: Number(values.timeout), gatewayOrigin: values.gateway, toolCycle: values["tool-cycle"], withNativeControl: values["with-native-control"] });
    console.log(JSON.stringify(result, null, 2));
    if (!result.ok) process.exitCode = 1;
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
