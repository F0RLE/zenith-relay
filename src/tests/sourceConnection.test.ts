import { expect, test } from "bun:test";
import { createInstance } from "i18next";
import { en } from "../src/i18n/locales/en";
import { sourceErrorDetails } from "../src/features/relay/pages/connections/sourceError";

const i18n = createInstance();
await i18n.init({ lng: "en", resources: { en: { translation: en } }, interpolation: { escapeValue: false } });

test("observed successful HTTP and validated WebSocket statuses are not source errors", () => {
  for (const status of [101, 200, 204, 307]) expect(sourceErrorDetails(status, i18n.t)).toBeNull();
});

test("connection diagnostics reuse the existing safe HTTP error reasons", () => {
  for (const status of [400, 401, 403, 404, 429, 500]) {
    const details = sourceErrorDetails(status, i18n.t)!;
    expect(details).toEqual(sourceErrorDetails(`HTTP ${status}`, i18n.t));
    expect(details.error.status).toBe(status);
    expect(details.summary).toContain(`HTTP ${status}`);
  }
});

test("transport failure and timeout remain distinct without a made-up status", () => {
  expect(sourceErrorDetails("connect", i18n.t)?.summary).toBe("Could not connect to the API");
  expect(sourceErrorDetails("timeout", i18n.t)?.summary).toBe("API request timed out");
  expect(sourceErrorDetails("timeout", i18n.t)?.error.status).toBeUndefined();
});
