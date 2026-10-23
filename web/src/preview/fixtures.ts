import type { ComponentId, RequestRecord, TokenCounters } from "../features/usage/model";
import { componentTokens } from "../features/usage/model";

const keys = [
  { id: "editor", name: "Редактор", mask: "demo_…7P2" },
  { id: "project", name: "Мой проект", mask: "demo_…9K4" },
  { id: "bot", name: "Бот", mask: "demo_…3B8" },
];
const models = [
  { id: "demo-gpt", name: "GPT · демо", reasoningEffort: "Высокое" },
  { id: "demo-claude", name: "Claude · демо", reasoningEffort: "Среднее" },
  { id: "demo-gemini", name: "Gemini · демо", reasoningEffort: "Не запрашивалось" },
];

// Synthetic fixture arithmetic only; real charges are supplied by the Hosted API.
export function priceDemoComponent(tokens: number | null, rate: bigint): bigint | null {
  if (rate < 0n || (tokens !== null && (!Number.isSafeInteger(tokens) || tokens < 0))) {
    throw new RangeError("Invalid synthetic token count or rate");
  }
  return tokens === null ? null : (BigInt(tokens) * rate + 500000n) / 1000000n;
}

export const previewRequests: RequestRecord[] = Array.from({ length: 36 }, (_, index): RequestRecord => {
  const family = index % 3;
  const running = index === 0 || index === 3 || index === 6;
  const pending = index === 1;
  const failed = index === 7 || index === 14;
  const known = !running && !pending;
  const input = 2420 + index * 60;
  const output = 435 + index * 10;
  const tokens: TokenCounters = {
    input: known ? input : null, output: known ? output : null,
    cacheRead: known ? 1064 : null, cacheWrite: known ? 84 : null,
    reasoning: known ? family === 2 ? 0 : 65 : null,
  };
  const rates: Record<ComponentId, bigint> = { input: 400000n, cacheRead: 100000n, cacheWrite: 500000n, output: 1200000n };
  const quantities = componentTokens(tokens);
  const components = {
    input: priceDemoComponent(quantities.input, rates.input),
    cacheRead: priceDemoComponent(quantities.cacheRead, rates.cacheRead),
    cacheWrite: priceDemoComponent(quantities.cacheWrite, rates.cacheWrite),
    output: priceDemoComponent(quantities.output, rates.output),
  };
  const firstTokenMs = 420 + family * 90;
  const generationMs = 7700 + index * 50;
  const day = index < 28 ? 20 : 20 - (index - 26) * 2;
  return {
    id: `demo_request_${String(index + 1).padStart(4, "0")}`,
    startedAt: new Date(Date.UTC(2026, 9, day, running ? 14 : 14 - Math.floor(index / 4), running ? 31 : 31 - (index % 4) * 7, 45 - index % 10)).toISOString(),
    model: models[family]!, key: keys[index % keys.length]!,
    execution: running ? "running" : failed ? "failed" : "completed",
    accounting: running ? "held" : pending ? "pending" : "posted",
    protocol: ["Responses", "Messages", "Gemini"][family]!,
    transport: family === 0 ? "WebSocket" : "HTTP", tier: "Обычная", tokens,
    timing: { firstTokenMs: running ? null : firstTokenMs, totalMs: running ? null : generationMs + firstTokenMs, generationMs: known ? generationMs : null },
    billing: { revision: `demo-${family}-v1`, rates, components,
      postedMicroUsd: known ? Object.values(components).reduce<bigint>((sum, amount) => sum + amount!, 0n) : null,
      reservedMicroUsd: running ? 60000n : 0n },
    client: { label: ["Demo Editor", "Demo App", "Demo Bot"][family]!, userAgent: `DemoClient/${family + 1}.0` },
    response: {
      httpStatus: running ? null : 200,
      result: running ? "Ответ ещё выполняется" : failed ? "Ответ прерван" : "Ответ завершён",
      errorCode: failed ? "response_interrupted" : null,
      guidance: failed ? "Поток ответа закрылся до подтверждения завершения. Списаны подтверждённые токены. Перед повтором проверьте результат в клиенте: действия могли выполниться. Сохраните ID запроса для поддержки." : null,
    },
    context: { bodyBytes: 28340 + index * 50, instructionBytes: 1240, conversationBytes: 23100 + index * 50, toolSchemaBytes: 3200 },
    tools: family === 0 ? { declared: 8, forwarded: 8, calls: known ? 2 : 0, choice: "Автоматически", result: running ? "Ожидается" : "Вызов инструмента" } : null,
  };
}).sort((left, right) => Date.parse(right.startedAt) - Date.parse(left.startedAt));
