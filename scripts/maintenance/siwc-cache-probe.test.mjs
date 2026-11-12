import { describe, expect, spyOn, test } from 'bun:test';
import { acceptEvent, configuration, httpTurn, nextInput, observation, report, safeError, syntheticRequest, websocketTurn } from './siwc-cache-probe.mjs';

describe('SIWC transport acceptance safety', () => {
  test('requires explicit inference opt-in and model/effort', () => {
    expect(() => configuration([])).toThrow();
    expect(() => configuration(['synthetic', 'high'])).toThrow();
    expect(() => configuration(['--allow-requests', 'synthetic', 'high', 'token'])).toThrow();
    expect(configuration(['--allow-requests', 'synthetic', 'high'])).toEqual({ model: 'synthetic', effort: 'high' });
  });

  test('arms differ only by a fixed-length cache isolation cohort', () => {
    const config = { model: 'synthetic', effort: 'high' };
    const first = syntheticRequest(config, 'aaaaaaaa');
    const second = syntheticRequest(config, 'bbbbbbbb');
    expect(JSON.stringify(first).replace('aaaaaaaa', 'bbbbbbbb')).toBe(JSON.stringify(second));
    expect(first.store).toBe(false);
    expect(first.reasoning.effort).toBe('high');
    expect(first.include).toEqual(['reasoning.encrypted_content']);
    expect(first.input[1].type).toBe('additional_tools');
    expect(first).not.toHaveProperty('prompt_cache_options');
  });

  test('missing usage remains unknown, not a fabricated zero', () => {
    const state = observation('http', 1, '{}');
    acceptEvent(state, { type: 'response.completed', response: { status: 'completed', output: [] } });
    expect(report(state).cachedTokens).toBeNull();
    expect(report(state).inputTokens).toBeNull();
  });

  test('first content timing excludes response lifecycle events', () => {
    const state = observation('websocket', 1, '{}');
    acceptEvent(state, { type: 'response.created' });
    expect(state.firstEventMs).not.toBeNull();
    expect(state.firstOutputMs).toBeNull();
    acceptEvent(state, { type: 'response.function_call_arguments.delta', delta: '{}' });
    expect(state.firstOutputMs).not.toBeNull();
  });

  test('reports counters but no bodies, response IDs or provider messages', () => {
    const state = observation('http', 1, '{}');
    acceptEvent(state, { type: 'response.failed', response: {
      id: 'private-response', output: [{ text: 'private-output' }],
      error: { code: 'rate_limit_exceeded', type: 'server_error', message: 'private-error' },
      usage: { input_tokens: 4000, input_tokens_details: { cached_tokens: 3712 }, output_tokens: 5 },
    } });
    expect(report(state).cachedTokens).toBe(3712);
    expect(JSON.stringify(report(state))).not.toContain('private');
    expect(() => acceptEvent(state, { type: 'response.completed' })).toThrow();
    expect(safeError({ code: 'Bearer private', param: 'private/value' })).toEqual({ code: null, type: null, param: null });
  });

  test('only the actual synthetic tool call can be fulfilled', () => {
    const response = { status: 'completed', output: [{ type: 'function_call', name: 'cache_probe', call_id: 'call_synthetic', arguments: '{}' }] };
    expect(nextInput(response, 1)).toEqual([{ type: 'function_call_output', call_id: 'call_synthetic', output: 'ok' }]);
    expect(() => nextInput({ ...response, status: 'incomplete' }, 1)).toThrow();
    expect(() => nextInput({ ...response, output: [...response.output, ...response.output] }, 1)).toThrow();
    expect(() => nextInput(response, 2)).toThrow();
  });

  test('HTTP parses fragmented SSE without exposing response content', async () => {
    const wire = 'data: {"type":"response.completed","response":{"id":"synthetic","status":"completed","output":[],"usage":{"input_tokens":42}}}\r\n\r\n';
    const encoded = new TextEncoder().encode(wire);
    const response = new Response(new ReadableStream({
      start(controller) {
        for (let offset = 0; offset < encoded.length; offset += 7) controller.enqueue(encoded.slice(offset, offset + 7));
        controller.close();
      },
    }));
    const fetch = spyOn(globalThis, 'fetch').mockResolvedValueOnce(response);
    try {
      const state = await httpTurn({}, { headers: {} }, 1);
      expect(state.terminal).toBe('response.completed');
      expect(report(state).inputTokens).toBe(42);
      expect(fetch).toHaveBeenCalledTimes(1);
    } finally { fetch.mockRestore(); }
  });

  test('WebSocket continuation and provider errors never retry', async () => {
    let sent = 0;
    const socket = { send(payload) {
      sent += 1;
      expect(JSON.parse(payload).previous_response_id).toBe('synthetic');
      queueMicrotask(() => socket.onmessage({ data: JSON.stringify({ type: 'error', error: { code: 'rate_limit_exceeded' } }) }));
    } };
    const state = await websocketTurn(socket, { previous_response_id: 'synthetic', input: [] }, 2);
    expect(state.terminal).toBe('error');
    expect(report(state).error.code).toBe('rate_limit_exceeded');
    expect(sent).toBe(1);
  });

  test('a closed WebSocket without a terminal fails without replay', async () => {
    let sent = 0;
    const socket = { send() { sent += 1; queueMicrotask(() => socket.onclose()); } };
    await expect(websocketTurn(socket, {}, 1)).rejects.toThrow('websocket_closed_without_terminal');
    expect(sent).toBe(1);
  });
});
