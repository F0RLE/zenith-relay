import { request } from "node:http";
import { Transform } from "node:stream";
import { pipeline } from "node:stream/promises";
import { StringDecoder } from "node:string_decoder";

const LIMIT = 512 * 1024;
const EVENTS = new Set([
  "response.created", "response.in_progress", "response.completed", "response.incomplete", "response.failed",
  "response.output_item.added", "response.output_item.done", "response.content_part.added", "response.content_part.done",
  "response.output_text.delta", "response.output_text.done", "response.function_call_arguments.delta",
  "response.function_call_arguments.done", "response.reasoning_summary_text.delta", "response.reasoning_summary_text.done",
]);

export function loopbackOrigin(value) {
  const url = new URL(value);
  if (url.protocol !== "http:" || url.hostname !== "127.0.0.1" || !url.port
    || url.pathname !== "/" || url.search || url.hash || url.username || url.password) {
    throw new Error("Select an explicit loopback HTTP origin");
  }
  return url.origin;
}

/** Forward bytes incrementally; retain only bounded, allowlisted observations. */
export async function forwardCodexGateway({ origin, bytes, response, timeoutMs, key, onTerminal = () => {} }) {
  origin = loopbackOrigin(origin);
  const start = performance.now();
  const elapsed = () => Math.round(performance.now() - start);
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  const closed = () => { if (!response.writableFinished) controller.abort(); };
  response.once("close", closed);
  try {
    const upstream = await new Promise((resolve, reject) => {
      const outgoing = request(new URL("/v1/responses", origin), { method: "POST",
        headers: { "content-type": "application/json", authorization: `Bearer ${key}` },
        signal: controller.signal }, resolve);
      outgoing.once("error", reject);
      outgoing.end(bytes);
    });
    const observed = { status: upstream.statusCode, headersMs: elapsed(), bytes: 0,
      eventTypes: {}, terminalType: null, terminalStatus: null, usage: null };
    const requestId = upstream.headers["x-zenith-relay-request-id"];
    if (typeof requestId === "string" && /^[A-Za-z0-9_-]{1,128}$/.test(requestId)) observed.requestId = requestId;
    const isSse = upstream.headers["content-type"]?.includes("text/event-stream");
    const decoder = new StringDecoder("utf8");
    let pending = "", data = [], terminals = 0;
    const frame = () => {
      if (!data.length) return;
      const text = data.join("\n");
      data = [];
      if (text === "[DONE]") return;
      let event;
      try { event = JSON.parse(text); } catch { throw new Error("Gateway emitted invalid SSE JSON"); }
      const type = EVENTS.has(event.type) ? event.type : "other";
      observed.eventTypes[type] = (observed.eventTypes[type] ?? 0) + 1;
      if (event.type === "response.output_text.delta" && typeof event.delta === "string" && event.delta.length) observed.firstTextMs ??= elapsed();
      if (event.type === "response.function_call_arguments.delta" && typeof event.delta === "string" && event.delta.length) observed.firstToolArgumentsMs ??= elapsed();
      if (["response.completed", "response.incomplete", "response.failed"].includes(event.type)) {
        if (++terminals !== 1) throw new Error("Gateway emitted multiple terminal responses");
        observed.terminalType = event.type;
        observed.terminalStatus = event.response?.status ?? null;
        const usage = event.response?.usage;
        if (usage) observed.usage = { inputTokens: usage.input_tokens ?? null,
          cachedInputTokens: usage.input_tokens_details?.cached_tokens ?? null, outputTokens: usage.output_tokens ?? null };
        onTerminal(event.response);
      }
    };
    const line = (value) => {
      value = value.replace(/\r$/, "");
      if (!value) frame();
      else if (value.startsWith("data:")) data.push(value.slice(5).replace(/^ /, ""));
    };
    const consume = (text) => {
      pending += text;
      let index;
      while ((index = pending.indexOf("\n")) !== -1) {
        line(pending.slice(0, index));
        pending = pending.slice(index + 1);
      }
    };
    const observer = new Transform({
      transform(chunk, _encoding, callback) {
        try {
          observed.firstByteMs ??= elapsed();
          observed.bytes += chunk.length;
          if (observed.bytes > LIMIT) throw new Error("Gateway reply exceeds qualification limit");
          if (isSse) consume(decoder.write(chunk));
          callback(null, chunk);
        } catch (error) { callback(error); }
      },
      flush(callback) {
        try {
          if (isSse) {
            consume(decoder.end());
            if (pending.length) line(pending);
            frame();
          }
          callback();
        } catch (error) { callback(error); }
      },
    });
    response.writeHead(upstream.statusCode, { "content-type": upstream.headers["content-type"] ?? "application/json" });
    response.flushHeaders();
    await pipeline(upstream, observer, response);
    observed.totalMs = elapsed();
    return observed;
  } finally {
    clearTimeout(timer);
    response.removeListener("close", closed);
  }
}
