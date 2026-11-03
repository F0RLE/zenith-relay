import { describe, expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import { Activity } from "../../src/components/Icons";
import { Chart } from "../../src/features/overview/OverviewChart";
import { chartCeiling, chartLinePath } from "../../src/features/overview/chart-display";
import type { OverviewBucket } from "../../src/features/overview/timeline";
import { LocaleProvider } from "../../src/i18n/LocaleProvider";

describe("Hosted chart presentation", () => {
  test("readable ceilings cover the data without modifying source values", () => {
    const values = Object.freeze([null, 15780, 100, 0]);
    expect(chartCeiling(values, "tokens")).toBe(20000);
    expect(values).toEqual([null, 15780, 100, 0]);
    expect(chartCeiling([76.88], "tok/s")).toBe(100);
    expect(chartCeiling([5], "requests")).toBe(6);
  });
  test("money axes keep distinct microUSD ticks, including tiny and empty ranges", () => {
    expect(chartCeiling([.008388], "USD")).toBe(.01);
    expect(chartCeiling([.004197], "USD")).toBe(.005);
    expect(chartCeiling([.000001], "USD")).toBe(.000002);
    for (const amount of [0, .000001, .000009, .000012, .004197, .008388, 1.5]) {
      const ceiling = chartCeiling([amount], "USD");
      expect(ceiling).toBeGreaterThanOrEqual(amount);
      expect(new Set([0, ceiling / 2, ceiling].map(value => value.toFixed(6))).size).toBe(3);
    }
  });
  test("missing intervals break paths while known zero observations remain", () => {
    expect(chartLinePath([2, null, 0, 4], 4)).toBe("M12.5,50 M62.5,100 L87.5,0 ");
    expect(chartLinePath([null, null], 1)).toBe("");
    expect(chartLinePath([], 1)).toBe("");
  });
  test("known zero is not presented as missing measurement", () => {
    const buckets: OverviewBucket[] = [{ startMs: 0, endMs: 1, interval: "00:00 — 00:01 UTC", rows: [] }];
    const render = (value: number | null) => renderToStaticMarkup(<LocaleProvider><Chart buckets={buckets} measure={() => value} icon={Activity} label="Requests" total="0" unit="requests" tone="requests" /></LocaleProvider>);
    expect(render(0)).not.toContain("No measurements");
    expect(render(0)).toContain('data-value="0"');
    expect(render(null)).toContain("No measurements");
    expect(render(null)).not.toContain('data-value="0"');
  });
});
