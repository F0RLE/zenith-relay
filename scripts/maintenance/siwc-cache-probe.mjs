import { randomUUID } from 'node:crypto';

const HTTP_URL = 'https://api.openai.com/v1/responses';
const WS_URL = 'wss://api.openai.com/v1/responses';
const TIMEOUT_MS = 90_000;
const MAX_RESPONSE_BYTES = 4 * 1024 * 1024;
const USAGE = 'usage: bun scripts/maintenance/siwc-cache-probe.mjs --allow-requests MODEL EFFORT\nSupply accessToken and optional proxyUrl as JSON on stdin. Runs six synthetic generations; never retries.';

export function configuration(args) {
  if (args.length !== 3 || args[0] !== '--allow-requests'
    || !/^[a-zA-Z0-9._-]{1,128}$/.test(args[1])
    || !/^[a-z]{1,16}$/.test(args[2])) {
    throw new Error('invalid_arguments');
  }
  return { model: args[1], effort: args[2] };
}

export function syntheticRequest({ model, effort }, cohort) {
  const reference = Array.from({ length: 384 }, (_, index) =>
    `Record ${index}: amber birch cedar delta elm fir grove hazel iris jade kelp lime.`).join('\n');
  return {
    model,
    store: false,
    reasoning: { effort },
    include: ['reasoning.encrypted_content'],
    instructions: 'Call cache_probe once, then reply exactly OK after its result. Never summarize the reference records.',
    input: [
      { role: 'developer', content: `Synthetic cache acceptance cohort ${cohort}. Reference records:\n${reference}` },
      { type: 'additional_tools', role: 'developer', tools: [{
        type: 'function', name: 'cache_probe', description: 'Read a synthetic acceptance result.',
        parameters: { type: 'object', properties: {}, required: [], additionalProperties: false },
        strict: true,
      }] },
      { role: 'user', content: 'Call cache_probe with empty arguments.' },
    ],
  };
}

export function safeError(error) {
  const atom = (value) => typeof value === 'string' && /^[a-zA-Z0-9_.-]{1,100}$/.test(value) ? value : null;
  return { code: atom(error?.code), type: atom(error?.type), param: atom(error?.param) };
}

export function observation(transport, turn, body) {
  return {
    transport, turn, sentBytes: Buffer.byteLength(body),
    started: performance.now(), firstEventMs: null, firstOutputMs: null,
    terminal: null, response: null,
  };
}

export function acceptEvent(state, event) {
  if (state.terminal) throw new Error('event_after_terminal');
  state.firstEventMs ??= Math.round(performance.now() - state.started);
  if (['response.output_text.delta', 'response.function_call_arguments.delta',
    'response.custom_tool_call_input.delta'].includes(event.type) && event.delta) {
    state.firstOutputMs ??= Math.round(performance.now() - state.started);
  }
  if (['response.completed', 'response.failed', 'response.incomplete', 'error'].includes(event.type)) {
    state.terminal = event.type;
    state.response = event.response ?? null;
    state.error = safeError(event.error ?? event.response?.error);
    state.elapsedMs = Math.round(performance.now() - state.started);
  }
}

export function report(state) {
  const usage = state.response?.usage;
  const counter = (value) => Number.isSafeInteger(value) && value >= 0 ? value : null;
  return {
    transport: state.transport, turn: state.turn, status: state.status ?? null,
    terminal: state.terminal, sentBytes: state.sentBytes,
    firstEventMs: state.firstEventMs, firstOutputMs: state.firstOutputMs,
    elapsedMs: state.elapsedMs ?? Math.round(performance.now() - state.started),
    inputTokens: counter(usage?.input_tokens),
    cachedTokens: counter(usage?.input_tokens_details?.cached_tokens),
    cacheWriteTokens: counter(usage?.input_tokens_details?.cache_write_tokens),
    outputTokens: counter(usage?.output_tokens),
    ...(state.terminal !== 'response.completed' ? { error: state.error ?? null } : {}),
  };
}

export function nextInput(response, turn) {
  if (response?.status !== 'completed' || !Array.isArray(response.output)) {
    throw new Error('completed_response_required');
  }
  if (turn === 1) {
    const calls = response.output.filter((item) => item.type === 'function_call');
    if (calls.length !== 1 || calls[0].name !== 'cache_probe'
      || !calls[0].call_id || JSON.stringify(JSON.parse(calls[0].arguments)) !== '{}') {
      throw new Error('expected_single_synthetic_tool_call');
    }
    return [{ type: 'function_call_output', call_id: calls[0].call_id, output: 'ok' }];
  }
  if (response.output.some((item) => item.type === 'function_call')) {
    throw new Error('unexpected_additional_tool_call');
  }
  return [{ role: 'user', content: 'Reply exactly OK again, without calling tools.' }];
}

export async function httpTurn(body, authorization, turn) {
  const encoded = JSON.stringify({ ...body, stream: true });
  const state = observation('http', turn, encoded);
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), TIMEOUT_MS);
  let reader;
  try {
    const response = await fetch(HTTP_URL, {
      method: 'POST', headers: authorization.headers, body: encoded,
      ...(authorization.proxyUrl ? { proxy: authorization.proxyUrl } : {}),
      redirect: 'error', signal: controller.signal,
    });
    state.status = response.status;
    reader = response.body?.getReader();
    if (!reader) throw new Error('missing_response_body');
    const decoder = new TextDecoder();
    let buffer = '';
    let received = 0;
    while (!state.terminal) {
      const { done, value } = await reader.read();
      if (done) break;
      received += value.byteLength;
      if (received > MAX_RESPONSE_BYTES) throw new Error('response_too_large');
      buffer += decoder.decode(value, { stream: true });
      if (!response.ok) continue;
      let end;
      while ((end = buffer.search(/\r?\n\r?\n/)) >= 0 && !state.terminal) {
        const frame = buffer.slice(0, end);
        buffer = buffer.slice(end).replace(/^\r?\n\r?\n/, '');
        const data = frame.split(/\r?\n/).filter((line) => line.startsWith('data:'))
          .map((line) => line.slice(5).trimStart()).join('\n');
        if (data && data !== '[DONE]') acceptEvent(state, JSON.parse(data));
      }
    }
    if (!response.ok) {
      let error;
      try { error = JSON.parse(buffer).error; } catch { /* Raw errors stay private. */ }
      acceptEvent(state, { type: 'error', error });
    }
    if (!state.terminal) throw new Error('stream_ended_without_terminal');
    return state;
  } finally {
    clearTimeout(timer);
    await reader?.cancel().catch(() => {});
  }
}

export async function openSocket(authorization) {
  const socket = new WebSocket(WS_URL, {
    headers: authorization.headers,
    ...(authorization.proxyUrl ? { proxy: authorization.proxyUrl } : {}),
  });
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => { socket.close(); reject(new Error('websocket_open_timeout')); }, 15_000);
    socket.onopen = () => { clearTimeout(timer); resolve(); };
    socket.onerror = () => { clearTimeout(timer); reject(new Error('websocket_open_failed')); };
    socket.onclose = () => { clearTimeout(timer); reject(new Error('websocket_closed_before_open')); };
  });
  return socket;
}

export async function websocketTurn(socket, body, turn) {
  const encoded = JSON.stringify({ type: 'response.create', ...body });
  const state = observation('websocket', turn, encoded);
  state.status = 101;
  return new Promise((resolve, reject) => {
    let received = 0;
    const finish = (error) => {
      clearTimeout(timer);
      socket.onmessage = null;
      socket.onerror = null;
      socket.onclose = null;
      if (error) reject(error); else resolve(state);
    };
    const timer = setTimeout(() => { finish(new Error('websocket_response_timeout')); socket.close(); }, TIMEOUT_MS);
    socket.onmessage = (event) => {
      try {
        if (typeof event.data !== 'string') throw new Error('unexpected_binary_frame');
        received += Buffer.byteLength(event.data);
        if (received > MAX_RESPONSE_BYTES) throw new Error('response_too_large');
        acceptEvent(state, JSON.parse(event.data));
        if (state.terminal) finish();
      } catch { finish(new Error('invalid_websocket_event')); }
    };
    socket.onerror = () => finish(new Error('websocket_transport_error'));
    socket.onclose = () => finish(new Error('websocket_closed_without_terminal'));
    try { socket.send(encoded); } catch { finish(new Error('websocket_send_failed')); }
  });
}

async function main() {
  if (process.argv.length === 3 && process.argv[2] === '--help') {
    console.log(USAGE);
    return;
  }
  const config = configuration(process.argv.slice(2));
  const input = await Bun.stdin.text();
  if (input.length > 128 * 1024) throw new Error('credential_input_too_large');
  const { accessToken, proxyUrl } = JSON.parse(input);
  if (typeof accessToken !== 'string' || !accessToken || /\s/.test(accessToken)) throw new Error('invalid_access_token');
  if (proxyUrl && !['http:', 'https:'].includes(new URL(proxyUrl).protocol)) throw new Error('unsupported_proxy');
  const authorization = {
    headers: { Authorization: `Bearer ${accessToken}`, 'Content-Type': 'application/json', 'User-Agent': 'Zenith-Relay-Cache-Acceptance' },
    proxyUrl,
  };
  const http = syntheticRequest(config, randomUUID());
  const ws = syntheticRequest(config, randomUUID());
  const socket = await openSocket(authorization);
  try {
    for (let turn = 1; turn <= 3; turn += 1) {
      for (const [transport, body] of [['http', http], ['websocket', ws]]) {
        const state = transport === 'http' ? await httpTurn(body, authorization, turn) : await websocketTurn(socket, body, turn);
        console.log(JSON.stringify(report(state)));
        if (state.terminal !== 'response.completed') throw new Error('provider_did_not_complete');
        const delta = nextInput(state.response, turn);
        if (transport === 'http') body.input.push(...state.response.output, ...delta);
        else {
          if (typeof state.response.id !== 'string' || !state.response.id) throw new Error('missing_continuation_id');
          body.previous_response_id = state.response.id;
          body.input = delta;
        }
      }
    }
  } finally { socket.close(); }
}

if (import.meta.main) {
  try { await main(); }
  catch (error) {
    const message = error?.message === 'invalid_arguments' ? USAGE
      : /^[a-z_]{1,80}$/.test(error?.message ?? '') ? error.message : 'probe_failed; no request was retried';
    console.error(message);
    process.exitCode = 1;
  }
}
