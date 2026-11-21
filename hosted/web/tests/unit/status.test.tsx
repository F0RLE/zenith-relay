import { describe, expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import { LocaleProvider } from "../../src/i18n/LocaleProvider";
import { StatusPage } from "../../src/features/status/StatusPage";
import { statusIntervals } from "../../src/features/status/history";
import { previewSnapshot } from "../../src/preview/snapshot";

describe("Model status presentation", () => {
  test("day and week windows cover the requested UTC period without gaps or future intervals", () => {
    const end = Date.parse("2026-11-01T14:32:00Z");
    for (const [period, count, duration] of [["day", 48, 86_400_000], ["week", 28, 604_800_000]] as const) {
      const intervals = statusIntervals(period, end);
      expect(intervals).toHaveLength(count);
      expect(intervals[0]!.startMs).toBe(end - duration);
      expect(intervals.at(-1)!.endMs).toBe(end);
      intervals.forEach((interval, index) => {
        expect(interval.endMs - interval.startMs).toBe(duration / count);
        expect(interval.endMs).toBeLessThanOrEqual(end);
        if (index) expect(interval.startMs).toBe(intervals[index - 1]!.endMs);
      });
    }
    expect(statusIntervals("day", NaN)).toEqual([]);
    expect(statusIntervals("week", Infinity)).toEqual([]);
  });

  test("catalog entries are unknown, not successful probes or zero-traffic evidence", () => {
    const html = renderToStaticMarkup(<LocaleProvider><StatusPage models={previewSnapshot.models} asOf={previewSnapshot.asOf} /></LocaleProvider>);
    const rows = html.match(/<tbody>([\s\S]*?)<\/tbody>/)![1]!;
    expect(rows.match(/class="model-status-row"/g)).toHaveLength(previewSnapshot.models.length);
    expect(rows).not.toMatch(/data-state="(?:operational|degraded|unavailable)"/);
    expect(rows).not.toMatch(/\d+%/);
    expect(rows).not.toContain("Primary API");
    expect(rows).not.toContain("Backup API");
    expect(rows.match(/tabindex="0"/g)).toHaveLength(previewSnapshot.models.length);
    expect(html).not.toContain("Awaiting monitoring data");
    expect(html).not.toContain('class="model-status-monitor"');
    expect(rows).not.toContain('class="model-status-state"');
    expect(rows).toContain('class="sr-only">No data</span>');
  });

  test("period switching belongs to the page heading and uses shared Relay tabs", () => {
    const html = renderToStaticMarkup(<LocaleProvider><StatusPage /></LocaleProvider>);
    const header = html.match(/<header class="page-heading status-heading">([\s\S]*?)<\/header>/)![1]!;
    expect(header).toContain("<h1>Status</h1>");
    expect(header).toContain('class="relay-tabs model-status-period"');
    expect(header.match(/<button /g)).toHaveLength(2);
    expect(header).toContain('class="active" aria-pressed="true"');
    expect(header).toContain('aria-pressed="false"');
    expect(html).not.toContain('class="model-status-controls"');
    expect(html).not.toContain("period-buttons");
  });

  test("one legend sits between the model count and search, before the table", () => {
    const html = renderToStaticMarkup(<LocaleProvider><StatusPage models={previewSnapshot.models} /></LocaleProvider>);
    expect(html.match(/class="model-status-legend"/g)).toHaveLength(1);
    const heading = html.indexOf('id="status-models-title"');
    const legend = html.indexOf('class="model-status-legend"');
    const search = html.indexOf('class="model-status-search"');
    const table = html.indexOf('class="table-shell model-status-frame"');
    expect(heading).toBeGreaterThanOrEqual(0);
    expect(legend).toBeGreaterThan(heading);
    expect(search).toBeGreaterThan(legend);
    expect(table).toBeGreaterThan(search);
  });

  test("an absent catalog does not manufacture rows or an outage", () => {
    const html = renderToStaticMarkup(<LocaleProvider><StatusPage /></LocaleProvider>);
    expect(html).toContain("No models available yet");
    expect(html).not.toContain('class="model-status-row"');
    expect(html).not.toContain('class="model-status-tooltip"');
  });
});
