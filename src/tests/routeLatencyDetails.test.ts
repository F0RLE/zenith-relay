import { describe, expect, test } from "bun:test";
import { createInstance } from "i18next";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import type { RouteLatencyDiagnostics } from "../src/features/relay/api/types/usage";
import { RouteLatencyDetails } from "../src/features/relay/pages/usage/RouteLatencyDetails";
import { en } from "../src/i18n/locales/en";
import { ru } from "../src/i18n/locales/ru";

const ready: RouteLatencyDiagnostics = {
  state: "ready", sampleCount: 20, unknownCount: 2, minimumSamples: 20,
  windowMs: 600_000, asOfMs: 1_000, lastObservedAtMs: 900,
  p50Ms: 0, p90Ms: 180, p95Ms: 190,
};

async function render(diagnostics: RouteLatencyDiagnostics | null | undefined, language = "en") {
  const i18n = createInstance();
  await i18n.init({
    lng: language, fallbackLng: "en", resources: { en: { translation: en }, ru: { translation: ru } },
    interpolation: { escapeValue: false },
  });
  return renderToStaticMarkup(createElement(I18nextProvider, { i18n },
    createElement("dl", null, createElement(RouteLatencyDetails, { diagnostics }))));
}

describe("route latency details", () => {
  test("shows measured quantiles and preserves a real zero", async () => {
    const html = await render(ready);
    expect(html).toContain("P50 0");
    expect(html).toContain("P90 180");
    expect(html).toContain("P95 190");
    expect(html).toContain("20 measured");
    expect(html).toContain("2 unmeasured");
    expect(html).toContain("Sample age at selection");
    expect(html).toContain("100");
  });

  test("keeps unknown, insufficient and stale samples distinct", async () => {
    for (const [state, label] of [
      ["unknown", "Awaiting measurements"],
      ["insufficient", "Collecting measurements (minimum 20)"],
      ["stale", "Measurements expired"],
    ] as const) {
      const html = await render({ ...ready, state, p50Ms: null, p90Ms: null, p95Ms: null });
      expect(html).toContain(label);
      expect(html).not.toContain("P50");
      expect(html).not.toContain("P95");
    }
  });

  test("supports existing logs and Russian labels", async () => {
    expect(await render(undefined)).toBe("<dl></dl>");
    expect(await render(null)).toBe("<dl></dl>");
    const html = await render({ ...ready, state: "unknown", lastObservedAtMs: null }, "ru");
    expect(html).toContain("Ожидание замеров");
    expect(html).toContain("2 без замера");
    expect(html).not.toContain("Возраст замера при выборе");
  });
});
