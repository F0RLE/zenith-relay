import { createServer } from "node:http";
import { mkdir, mkdtemp, realpath, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { isAbsolute, join, relative, sep } from "node:path";
import { createHash, randomBytes } from "node:crypto";
import { parseArgs } from "node:util";
import { fileURLToPath } from "node:url";
import { runClient } from "./check-claude-effort-wire.mjs";
import { FIXTURE_TOOL, FIXTURE_RESULT, fixtureResults, PARALLEL_FIXTURE_RESULT } from "./fixtures/claude-tool-server.mjs";
import { forwardCodexGateway, loopbackOrigin } from "./fixtures/codex-gateway-forward.mjs";
import { readCheckedText } from "../lib/checked-file.mjs";

const ACK = "relay-codex-contract-ok";
const KEY = "sk-relay-synthetic-codex-contract";
const LIMIT = 512 * 1024;
const CALL_ID = "call_relay_codex_fixture";
const RESPONSE_ID = "resp_relay_codex_tool";

export function isolatedCodexEnvironment(root, origin, parent = process.env) {
  const env = {};
  for (const key of ["SystemRoot", "SYSTEMROOT", "WINDIR", "COMSPEC", "PATH", "PATHEXT", "SystemDrive"]) {
    if (parent[key]) env[key] = parent[key];
  }
  return { ...env, HOME: root, USERPROFILE: root, APPDATA: join(root, "roaming"),
    LOCALAPPDATA: join(root, "local"), TEMP: root, TMP: root, CODEX_HOME: join(root, "codex"),
    RELAY_CODEX_FIXTURE_KEY: KEY, HTTP_PROXY: origin, HTTPS_PROXY: origin, ALL_PROXY: origin,
    NO_PROXY: "127.0.0.1", no_proxy: "127.0.0.1", TERM: "dumb" };
}

function fixtureTool(body, name = "lookup") {
  for (const tool of body.tools ?? []) {
    if (tool.type === "function" && tool.name === `mcp__relay_contract__${name}`) return { name: tool.name };
    if (tool.type === "namespace" && tool.name === "mcp__relay_contract"
      && tool.tools?.some(child => child.type === "function" && child.name === name)) {
      return { name, namespace: tool.name };
    }
  }
  return null;
}

function fixtureResult(value, expected = FIXTURE_RESULT) {
  if (value === expected) return true;
  if (typeof value === "string") {
    try { return fixtureResult(JSON.parse(value), expected); } catch { return false; }
  }
  if (Array.isArray(value)) return value.length > 0
    && value.every(part => ["text", "input_text"].includes(part?.type) && typeof part.text === "string")
    && value.some(part => fixtureResult(part.text, expected));
  return value?.isError !== true && Array.isArray(value?.content) && fixtureResult(value.content, expected);
}

function resultDigest(value) {
  const parts = typeof value === "string" ? [{ type: "string", text: value }] : value;
  const hash = createHash("sha256");
  for (const part of parts) hash.update(`${part.type}\0${Buffer.byteLength(part.text)}:`).update(part.text);
  return hash.digest("hex");
}

function toolResult(body, identity, original = { call_id: CALL_ID, response_id: RESPONSE_ID, arguments: '{"value":"synthetic"}' }, expected = FIXTURE_RESULT) {
  const items = Array.isArray(body.input) ? body.input : [];
  const calls = items.filter(item => item.type === "function_call" && item.call_id === original?.call_id);
  const results = items.filter(item => item.type === "function_call_output" && item.call_id === original?.call_id);
  if (results.length !== 1 || !fixtureResult(results[0].output, expected)) return false;
  if (!calls.length) return typeof original?.response_id === "string" && original.response_id.length > 0
    && body.previous_response_id === original.response_id;
  return calls.length === 1 && calls[0].name === identity.name
    && (calls[0].namespace ?? null) === (identity.namespace ?? null) && calls[0].arguments === original?.arguments
    && items.indexOf(results[0]) > items.indexOf(calls[0]);
}

function resultEvidence(body, identity, callId = CALL_ID, args = '{"value":"synthetic"}', expected = FIXTURE_RESULT) {
  const items = Array.isArray(body.input) ? body.input : [];
  const call = items.find(item => item.type === "function_call" && item.call_id === callId);
  const result = items.find(item => item.type === "function_call_output" && item.call_id === callId);
  let output = result?.output;
  if (typeof output === "string") { try { output = JSON.parse(output); } catch {} }
  return { callPresent: Boolean(call), resultPresent: Boolean(result),
    nameMatches: call?.name === identity?.name,
    namespaceMatches: (call?.namespace ?? null) === (identity?.namespace ?? null),
    argumentsMatch: call?.arguments === args,
    outputType: Array.isArray(output) ? "array" : typeof output,
    outputKeys: output && !Array.isArray(output) && typeof output === "object"
      ? Object.keys(output).map(key => ["result", "content", "type", "text", "isError", "structuredContent"].includes(key) ? key : "other").slice(0, 12) : [],
    outputPartTypes: Array.isArray(output) ? output.slice(0, 8).map(part => ["text", "input_text"].includes(part?.type) ? part.type : "other") : [],
    fixtureTextPresent: fixtureResult(result?.output, expected) };
}

function childDiagnostic(text, root, token, liveKey) {
  if (liveKey) text = text.replaceAll(liveKey, "<gateway-key>");
  return text.replaceAll(KEY, "<synthetic-key>").replaceAll(token, "<fixture-token>")
    .replaceAll(JSON.stringify(root).slice(1, -1), "<temporary-profile>")
    .replaceAll(root, "<temporary-profile>").slice(0, 2000);
}

function catalogEvidence(body, expected) {
  const texts = typeof body.instructions === "string" ? [body.instructions] : [];
  const roles = [];
  for (const item of Array.isArray(body.input) ? body.input : []) {
    if (!["system", "developer"].includes(item.role)) continue;
    roles.push(item.role);
    if (typeof item.content === "string") texts.push(item.content);
    else for (const part of item.content ?? []) if (typeof part?.text === "string") texts.push(part.text);
  }
  return { adopted: texts.some(text => text.includes(expected)), rootType: typeof body.instructions,
    rootLength: typeof body.instructions === "string" ? body.instructions.length : 0,
    expectedLength: expected.length, roles: roles.slice(0, 8) };
}

function responseEvents(model, identity) {
  const tool = Boolean(identity);
  const item = tool ? { type: "function_call", id: "fc_relay_fixture", call_id: CALL_ID,
    ...identity, arguments: '{"value":"synthetic"}', status: "completed" }
    : { type: "message", id: "msg_relay_fixture", role: "assistant", status: "completed",
      content: [{ type: "output_text", text: ACK, annotations: [] }] };
  const response = { id: tool ? RESPONSE_ID : "resp_relay_codex_answer", object: "response",
    created_at: 1, model, status: "completed", output: [item],
    usage: { input_tokens: 5, output_tokens: 5, total_tokens: 10,
      input_tokens_details: { cached_tokens: 0 }, output_tokens_details: { reasoning_tokens: 0 } } };
  const events = [
    ["response.created", { response: { ...response, status: "in_progress", output: [] } }],
    ["response.output_item.added", { output_index: 0, item: tool ? { ...item, arguments: "", status: "in_progress" } : { ...item, content: [], status: "in_progress" } }],
  ];
  if (tool) events.push(
    ["response.function_call_arguments.delta", { output_index: 0, item_id: item.id, delta: item.arguments }],
    ["response.function_call_arguments.done", { output_index: 0, item_id: item.id, arguments: item.arguments }],
  );
  else events.push(
    ["response.content_part.added", { output_index: 0, content_index: 0, item_id: item.id, part: { type: "output_text", text: "", annotations: [] } }],
    ["response.output_text.delta", { output_index: 0, content_index: 0, item_id: item.id, delta: ACK }],
    ["response.output_text.done", { output_index: 0, content_index: 0, item_id: item.id, text: ACK }],
    ["response.content_part.done", { output_index: 0, content_index: 0, item_id: item.id, part: item.content[0] }],
  );
  events.push(["response.output_item.done", { output_index: 0, item }], ["response.completed", { response }]);
  return events.map(([type, value], index) => `event: ${type}\ndata: ${JSON.stringify({ type, sequence_number: index, ...value })}\n\n`).join("");
}

function completion(stdout, acknowledgement = ACK) {
  try {
    const events = stdout.trim().split("\n").filter(Boolean).map(line => JSON.parse(line));
    return { ok: events.filter(event => event.type === "turn.completed").length === 1
      && !events.some(event => event.type === "turn.failed" || event.type === "error")
      && events.some(event => event.type === "item.completed" && event.item?.type === "agent_message" && event.item.text === acknowledgement)
      && !events.some(event => event.type === "item.completed" && ["command_execution", "file_change", "web_search"].includes(event.item?.type)),
      eventTypes: [...new Set(events.map(event => event.type))] };
  } catch { return { ok: false, eventTypes: [] }; }
}

export async function qualifyCodexWire({ cliPath, catalog, effort = "low", gatewayOrigin,
  timeoutMs = 30000, client = runClient, live, scenario = "single" }) {
  const expected = fixtureResults(scenario), names = Object.keys(expected);
  if (scenario === "parallel" && !live) throw new Error("Parallel client qualification requires explicit live-gateway mode");
  if (live !== undefined && (live?.allowProviderRequests !== true || typeof live?.key !== "string" || !live.key.trim() || !gatewayOrigin)) {
    throw new Error("Live qualification requires explicit provider-request consent and an authenticated loopback gateway");
  }
  if (!isAbsolute(cliPath) || !(await stat(cliPath)).isFile()) throw new Error("Select an absolute installed Codex executable");
  cliPath = await realpath(cliPath);
  if (!Array.isArray(catalog?.models) || catalog.models.length !== 1
    || !/^[A-Za-z0-9._/:=-]{1,256}$/.test(catalog.models[0]?.slug ?? "")) throw new Error("Supply one generated Codex catalog row");
  if (typeof catalog.models[0].base_instructions !== "string" || !catalog.models[0].base_instructions.trim()) throw new Error("The generated row must retain its base instructions");
  if (typeof effort !== "string" || !/^[a-z][a-z0-9_]{0,31}$/.test(effort)) throw new Error("Select an explicit fixture effort");
  if (!catalog.models[0].supported_reasoning_levels?.some(level => level.effort === effort)) throw new Error("The generated row must declare the requested effort");
  if (!Number.isSafeInteger(timeoutMs) || timeoutMs < 1000 || timeoutMs > 60000) throw new Error("Invalid fixture timeout");
  const gateway = gatewayOrigin === undefined ? null : loopbackOrigin(gatewayOrigin);
  const model = catalog.models[0].slug;
  const root = await realpath(await mkdtemp(join(tmpdir(), "relay-codex-wire-")));
  const token = randomBytes(32).toString("hex");
  const wire = { scenario, requests: [], generationRequests: 0, toolCalls: 0, unexpectedRequests: 0, unexpectedMethods: {}, blockedDestinations: {}, invalidRequests: 0, toolResultReceived: false };
  if (gateway) wire.responses = [];
  let identities = null;
  let originals = live ? null : [{ call_id: CALL_ID, response_id: RESPONSE_ID, arguments: '{"value":"synthetic"}' }];
  const audited = new Set();
  let cycleStarted, responseFinished, auditRejected = false;
  const server = createServer(async (request, response) => {
    try {
      if (request.url === "/fixture/tool-call" && request.method === "POST"
        && request.headers["x-relay-fixture-token"] === token && wire.generationRequests === 1) {
        const name = request.headers["x-relay-fixture-tool"] ?? (scenario === "single" ? "lookup" : null);
        if (names.includes(name) && !audited.has(name)) {
          audited.add(name); wire.toolCalls++;
          response.writeHead(204); response.end(); return;
        }
      }
      if (request.url !== "/v1/responses" || request.method !== "POST"
        || request.headers.authorization !== `Bearer ${KEY}`) {
        if (request.url === "/fixture/tool-call") auditRejected = true;
        wire.unexpectedRequests++;
        const method = ["GET", "POST", "PUT", "DELETE", "OPTIONS", "HEAD"].includes(request.method) ? request.method : "other";
        wire.unexpectedMethods[method] = (wire.unexpectedMethods[method] ?? 0) + 1;
        response.writeHead(400); response.end(); return;
      }
      const index = wire.generationRequests++;
      if (index === 1 && responseFinished !== undefined) wire.betweenRequestsMs = Math.round(performance.now() - responseFinished);
      if (index >= 8) { wire.invalidRequests++; request.resume(); response.writeHead(400); response.end(); return; }
      const chunks = []; let length = 0;
      for await (const chunk of request) {
        length += chunk.length;
        if (length > LIMIT) throw new Error("Fixture request exceeds its limit");
        chunks.push(chunk);
      }
      const bytes = Buffer.concat(chunks), body = JSON.parse(bytes.toString("utf8"));
      identities ??= names.map(name => fixtureTool(body, name));
      const declared = identities.every(Boolean);
      const evidence = catalogEvidence(body, catalog.models[0].base_instructions);
      const catalogAdopted = evidence.adopted;
      if (index === 1) {
        const evidence = names.map((name, slot) => resultEvidence(body, identities[slot], originals?.[slot]?.call_id, originals?.[slot]?.arguments, expected[name]));
        wire.resultEvidence = scenario === "single" ? evidence[0] : evidence;
      }
      const results = Array.isArray(body.input) ? body.input.filter(item => item.type === "function_call_output") : [];
      const valid = body.model === model && body.stream === true && body.reasoning?.effort === effort
        && !auditRejected && catalogAdopted && declared
        && (scenario !== "parallel" || body.parallel_tool_calls === undefined || body.parallel_tool_calls === true)
        && (index === 0 || (index === 1 && wire.toolCalls === names.length && results.length === names.length
          && names.every((name, slot) => toolResult(body, identities[slot], originals?.[slot], expected[name]))));
      wire.requests.push({ model: body.model === model ? model : "unexpected-model",
        effort: body.reasoning?.effort === effort ? effort : "unexpected-effort",
        catalogAdopted, catalogEvidence: evidence, toolDeclared: declared,
        parallelToolCalls: typeof body.parallel_tool_calls === "boolean" ? body.parallel_tool_calls : null });
      if (!valid) { wire.invalidRequests++; response.writeHead(400); response.end('{"error":{"message":"Invalid synthetic tool cycle"}}'); return; }
      if (index === 1) {
        wire.toolResultReceived = true;
        const digests = originals.map(original => resultDigest(results.find(item => item.call_id === original.call_id).output));
        if (scenario === "single") wire.toolResultDigest = digests[0];
        else wire.toolResultDigests = digests;
      }
      if (gateway) {
        const observed = await forwardCodexGateway({ origin: gateway, bytes, response, timeoutMs, key: live ? live.key : KEY,
          onTerminal(terminal) {
            if (!live) return;
            if (terminal?.status !== "completed") throw new Error("Live response did not complete");
            const allowed = index === 0 ? ["message", "reasoning", "function_call"] : ["message", "reasoning"];
            if (!Array.isArray(terminal.output) || terminal.output.some(item => !allowed.includes(item?.type))) {
              throw new Error("Live response contains an unrelated executable output");
            }
            if (index === 0) {
              const calls = terminal.output?.filter(item => item.type === "function_call") ?? [];
              if (calls.length !== names.length || new Set(calls.map(call => call.call_id)).size !== names.length) throw new Error("Live fixture call count or identity is invalid");
              originals = identities.map(identity => {
                const matching = calls.filter(call => call.name === identity.name && (call.namespace ?? null) === (identity.namespace ?? null));
                const call = matching[0];
                let args;
                try { args = JSON.parse(call?.arguments); } catch { throw new Error("Live fixture arguments are invalid"); }
                if (matching.length !== 1 || typeof call.call_id !== "string" || !call.call_id.trim()
                  || args?.value !== "synthetic" || Object.keys(args).length !== 1) throw new Error("Live response did not select the exact fixture tools");
                return { ...call, response_id: terminal.id };
              });
            } else {
              const text = terminal.output.flatMap(item => item.type === "message" ? item.content ?? [] : [])
                .filter(part => part.type === "output_text").map(part => part.text ?? "").join("");
              if (text !== (scenario === "parallel" ? PARALLEL_FIXTURE_RESULT : FIXTURE_RESULT)) {
                throw new Error("Live provider acknowledgement does not match the audited results");
              }
            }
          },
        });
        wire.responses.push(observed);
        responseFinished = performance.now();
        return;
      }
      response.writeHead(200, { "content-type": "text/event-stream" });
      response.end(responseEvents(model, index === 0 ? identities[0] : null));
    } catch { wire.invalidRequests++; if (!response.headersSent) response.writeHead(400); response.end(); }
  });
  server.on("connect", (request, socket) => {
    wire.unexpectedRequests++;
    wire.unexpectedMethods.CONNECT = (wire.unexpectedMethods.CONNECT ?? 0) + 1;
    let host = "other";
    try {
      const hostname = new URL(`https://${request.url}`).hostname;
      if (/(^|\.)(openai\.com|chatgpt\.com|statsigapi\.net)$/.test(hostname)) host = hostname;
    } catch {}
    if (!(host in wire.blockedDestinations) && Object.keys(wire.blockedDestinations).length >= 8) host = "other";
    wire.blockedDestinations[host] = (wire.blockedDestinations[host] ?? 0) + 1;
    socket.destroy();
  });
  try {
    await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
    const origin = `http://127.0.0.1:${server.address().port}`;
    const env = isolatedCodexEnvironment(root, origin);
    await mkdir(env.CODEX_HOME, { recursive: true });
    await writeFile(join(root, "catalog.json"), JSON.stringify(catalog));
    const version = await client({ cliPath, args: ["--version"], cwd: root, env, timeoutMs: 10000 });
    if (version.code !== 0 || version.failure || !/^codex-cli [A-Za-z0-9.+_-]{1,80}$/.test(version.stdout.trim())) throw new Error("Installed Codex version is unavailable");
    const help = await client({ cliPath, args: ["exec", "--help"], cwd: root, env, timeoutMs: 10000 });
    for (const flag of ["--strict-config", "--ignore-user-config", "--ignore-rules", "--ephemeral", "--sandbox", "--json", "--skip-git-repo-check"]) {
      if (help.code !== 0 || help.failure || !help.stdout.includes(flag)) throw new Error(`Installed client requires ${flag}`);
    }
    const q = JSON.stringify;
    const fixture = fileURLToPath(new URL("./fixtures/claude-tool-server.mjs", import.meta.url));
    const settings = [
      'model_provider="relay_contract"', `model_catalog_json=${q(join(root, "catalog.json"))}`,
      `model_reasoning_effort=${q(effort)}`, 'approval_policy="never"', "project_doc_max_bytes=0",
      "analytics.enabled=false", "feedback.enabled=false",
      "check_for_update_on_startup=false", `chatgpt_base_url=${q(origin)}`,
      "features.apps=false", "features.connectors=false", "features.plugins=false",
      "features.remote_plugin=false", "features.plugin_discovery_prefetch=false",
      "features.recommended_plugins=false", "features.remote_models=false", "features.api_key_model_discovery=false",
      `model_providers.relay_contract={name="Relay synthetic fixture",base_url=${q(`${origin}/v1`)},env_key="RELAY_CODEX_FIXTURE_KEY",wire_api="responses",requires_openai_auth=false,supports_websockets=false}`,
      `mcp_servers.relay_contract={command=${q(process.execPath)},args=[${q(fixture)}],env={RELAY_TOOL_FIXTURE_ORIGIN=${q(origin)},RELAY_TOOL_FIXTURE_TOKEN=${q(token)},RELAY_TOOL_FIXTURE_SCENARIO=${q(scenario)}},enabled=true}`,
    ];
    const prompt = scenario === "parallel"
      ? `In one response, call both independent read-only tools ${names.map(name => `mcp__relay_contract__${name}`).join(" and ")} exactly once each with value synthetic. Issue both calls together before waiting for either result. After both results, reply with exactly ${PARALLEL_FIXTURE_RESULT}. Do not read files or use other tools.`
      : live ? `Call ${FIXTURE_TOOL} exactly once with value synthetic. Then reply with exactly its result ${FIXTURE_RESULT}. Do not read files or use other tools.`
      : "Call the supplied synthetic lookup tool, then return the gateway acknowledgement.";
    cycleStarted = performance.now();
    const child = await client({ cliPath, args: ["exec", "--strict-config", "--ignore-user-config", "--ignore-rules", "--ephemeral", "--skip-git-repo-check", "--sandbox", "read-only", "--json", "--model", model,
      ...settings.flatMap(value => ["-c", value]), prompt], cwd: root, env, timeoutMs });
    const completed = completion(child.stdout, scenario === "parallel" ? PARALLEL_FIXTURE_RESULT : live ? FIXTURE_RESULT : ACK);
    const clientWarnings = { temporaryHelperAlias: child.stderr.includes("Refusing to create helper binaries under temporary dir"),
      fallbackModelMetadata: /fallback (?:model )?metadata/i.test(child.stderr),
      unknownConfiguration: /(?:unrecognized|unknown) (?:config|feature)|unknown field/i.test(child.stderr) };
    return { clientVersion: version.code === 0 ? version.stdout.trim() : "unknown", mode: live ? "live_gateway" : gateway ? "relay_gateway" : "direct_fixture",
      ...wire, clientCycleMs: Math.round(performance.now() - cycleStarted), completion: completed, clientWarnings, exitCode: child.code, failure: child.failure,
      stderr: child.code === 0 ? "" : childDiagnostic(child.stderr, root, token, live?.key),
      ok: version.code === 0 && !version.failure && child.code === 0 && !child.failure && completed.ok
        && !clientWarnings.fallbackModelMetadata && !clientWarnings.unknownConfiguration
        && wire.generationRequests === 2 && wire.requests.length === 2 && wire.toolCalls === names.length && wire.toolResultReceived
        && wire.unexpectedRequests === 0 && wire.invalidRequests === 0
        && (!gateway || (wire.responses.length === 2 && wire.responses.every(item => item.status === 200 && item.terminalType === "response.completed" && item.terminalStatus === "completed"))) };
  } finally {
    server.closeAllConnections?.();
    if (server.listening) await new Promise(resolve => server.close(resolve));
    const resolved = await realpath(root), location = relative(await realpath(tmpdir()), resolved);
    if (resolved !== root || isAbsolute(location) || location === ".." || location.startsWith(`..${sep}`) || !location.startsWith("relay-codex-wire-")) throw new Error("Temporary probe cleanup containment failed");
    await rm(resolved, { recursive: true });
  }
}

export async function liveCodexProfile({ profile, model, effort, allowProviderRequests }) {
  if (allowProviderRequests !== true || !isAbsolute(profile ?? "") || !model || !effort) {
    throw new Error("Live qualification requires --allow-provider-requests, --profile ABSOLUTE_PATH, --model and --effort");
  }
  let config, catalog;
  try {
    const profileText = readCheckedText(profile, undefined, { maxBytes: LIMIT });
    if (profileText === null) throw new Error();
    config = Bun.TOML.parse(profileText);
    if (!isAbsolute(config.model_catalog_json ?? "")) throw new Error();
    const catalogText = readCheckedText(config.model_catalog_json, undefined, { maxBytes: 16 * 1024 * 1024 });
    if (catalogText === null) throw new Error();
    catalog = JSON.parse(catalogText);
  } catch { throw new Error("Cannot read the selected Codex profile and generated catalog safely"); }
  const provider = config.model_providers?.[config.model_provider];
  let url;
  try { url = new URL(provider?.base_url); } catch { throw new Error("The selected profile has no valid Relay URL"); }
  const gatewayOrigin = loopbackOrigin(url.origin);
  if (url.pathname !== "/v1" || url.search || url.hash || url.username || url.password
    || typeof provider.experimental_bearer_token !== "string" || !provider.experimental_bearer_token.trim()) {
    throw new Error("The selected profile must use an authenticated loopback Relay /v1 provider");
  }
  const selected = catalog.models?.filter(row => row.slug === model);
  if (selected?.length !== 1 || !selected[0].supported_reasoning_levels?.some(level => level.effort === effort)) {
    throw new Error("The generated catalog must declare exactly the selected model and effort");
  }
  return { catalog: { models: selected }, effort, gatewayOrigin, live: { allowProviderRequests: true, key: provider.experimental_bearer_token } };
}

if (import.meta.main) {
  try {
    const { values } = parseArgs({ options: { cli: { type: "string" }, catalog: { type: "string" }, effort: { type: "string" }, gateway: { type: "string" },
      profile: { type: "string" }, model: { type: "string" }, scenario: { type: "string", default: "single" }, "allow-provider-requests": { type: "boolean", default: false } } });
    if (!values.cli) throw new Error("Supply --cli ABSOLUTE_PATH");
    if ((values.profile || values["allow-provider-requests"]) && (values.catalog || values.gateway)) throw new Error("Select live-profile or synthetic-gateway qualification, not both");
    if (values.model && !values.profile) throw new Error("Select --profile for live --model qualification");
    const options = values.profile || values["allow-provider-requests"]
      ? await liveCodexProfile({ profile: values.profile, model: values.model, effort: values.effort, allowProviderRequests: values["allow-provider-requests"] })
      : { catalog: JSON.parse(values.catalog ?? "null"), effort: values.effort, gatewayOrigin: values.gateway };
    const result = await qualifyCodexWire({ cliPath: values.cli, ...options, scenario: values.scenario });
    console.log(JSON.stringify(result, null, 2));
    if (!result.ok) process.exitCode = 1;
  } catch (error) {
    console.error(error.code && /^[A-Z_0-9]{1,64}$/.test(error.code)
      ? `Codex wire probe failed (${error.code})`
      : error instanceof SyntaxError ? "Generated catalog JSON is invalid" : error.message);
    process.exitCode = 1;
  }
}
