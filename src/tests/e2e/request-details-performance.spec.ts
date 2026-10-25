import { expect, test, type Page } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

async function trackDateFormatting(page: Page) {
  await page.addInitScript(() => {
    const state = window as unknown as { __REQUEST_DATE_FORMATS__: number };
    state.__REQUEST_DATE_FORMATS__ = 0;
    const descriptor = Object.getOwnPropertyDescriptor(Intl.DateTimeFormat.prototype, "format")!;
    Object.defineProperty(Intl.DateTimeFormat.prototype, "format", {
      ...descriptor,
      get(this: Intl.DateTimeFormat) {
        const format = descriptor.get!.call(this) as Intl.DateTimeFormat["format"];
        return (value?: Date | number) => { state.__REQUEST_DATE_FORMATS__ += 1; return format(value); };
      },
    });
  });
}

const dateFormats = (page: Page) => page.evaluate(() => (window as unknown as { __REQUEST_DATE_FORMATS__: number }).__REQUEST_DATE_FORMATS__);
const usageReads = (page: Page) => page.evaluate(() => (window as unknown as {
  __TAURI_TEST_INVOKES__: Array<{ command: string }>;
}).__TAURI_TEST_INVOKES__.filter(({ command }) => command === "get_local_usage_page").length);

for (const view of ["Sends", "Errors"] as const) {
  test(`closing request details leaves the 50-row ${view} table untouched while native detail reads are pending`, async ({ page }) => {
    await trackDateFormatting(page);
    await installTauriMock(page, {
      locale: "en",
      usageFilterHistory: Array.from({ length: 60 }, () => ({
        wireApi: "responses", transport: "http", errorCategory: "upstream_failure",
      })),
    });
    await page.goto("/");
    await page.getByRole("button", { name: "Usage", exact: true }).click();
    if (view === "Errors") await page.getByRole("tab", { name: view, exact: true }).click();
    const table = page.locator(view === "Sends" ? ".usage-request-table" : ".usage-error-table");
    await expect(table.locator("tbody tr")).toHaveCount(50);
    await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
    await page.evaluate(() => {
      const target = window as unknown as {
        __TAURI_INTERNALS__: { invoke: (name: string, args?: unknown, options?: unknown) => Promise<unknown> };
        __RELEASE_DETAIL_READS__: () => void;
      };
      const internals = target.__TAURI_INTERNALS__;
      const invoke = internals.invoke.bind(internals);
      const pending: Array<() => void> = [];
      target.__RELEASE_DETAIL_READS__ = () => pending.splice(0).forEach((release) => release());
      internals.invoke = (name, args, options) => name === "get_diagnostic_settings" || name === "get_local_cache_sessions"
        ? new Promise((resolve, reject) => pending.push(() => { void invoke(name, args, options).then(resolve, reject); }))
        : invoke(name, args, options);
    });
    const opener = table.locator(".request-link").first();
    for (const method of ["button", "escape", "backdrop"] as const) {
      const beforeOpen = await dateFormats(page);
      const readsBeforeOpen = await usageReads(page);
      await opener.click();
      const dialog = page.getByRole("dialog", { name: "Request details", exact: true });
      await expect(dialog).toBeVisible();
      expect(await dateFormats(page)).toBe(beforeOpen + 1);
      expect(await usageReads(page)).toBe(readsBeforeOpen);
      // Closed diagnostics should not create their DOM, tooltips or focus targets.
      await expect(dialog.locator(".request-technical-grid")).toHaveCount(0);
      await dialog.locator(".request-technical-details > summary").click();
      await expect(dialog.locator(".request-id-list")).toBeVisible();
      await dialog.locator(".request-technical-details > summary").click();
      await expect(dialog.locator(".request-technical-grid")).toHaveCount(0);
      await expect(page.locator(".request-details-backdrop")).toHaveCSS("backdrop-filter", "none");
      const before = await dateFormats(page);
      const reads = await usageReads(page);
      if (method === "button") await dialog.getByRole("button", { name: "Close", exact: true }).click();
      else if (method === "escape") await page.keyboard.press("Escape");
      else await page.locator(".relay-modal-backdrop").click({ position: { x: 5, y: 5 } });
      await expect(dialog).toHaveCount(0);
      await expect(opener, `focus after ${method}`).toBeFocused();
      expect(await dateFormats(page)).toBe(before);
      expect(await usageReads(page)).toBe(reads);
      await expect(table.locator("tbody tr")).toHaveCount(50);
    }
    const beforeLateReads = await dateFormats(page);
    await page.evaluate(async () => {
      (window as unknown as { __RELEASE_DETAIL_READS__: () => void }).__RELEASE_DETAIL_READS__();
      await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    });
    await expect(page.getByRole("dialog")).toHaveCount(0);
    expect(await dateFormats(page)).toBe(beforeLateReads);
  });
}

for (const newFocus of [false, true]) {
  test(`request details disappear before passive focus work; newer focus wins=${newFocus}`, async ({ page }) => {
    await installTauriMock(page, { locale: "en" });
    await page.goto("/");
    await page.getByRole("button", { name: "Usage", exact: true }).click();
    const opener = page.locator(".usage-request-table .request-link").first();
    await opener.click();
    await expect(page.getByRole("dialog")).toBeVisible();
    await page.evaluate(async () => {
      await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
      const original = window.requestAnimationFrame;
      const originalCancel = window.cancelAnimationFrame;
      const callbacks = new Map<number, FrameRequestCallback>();
      let next = 0;
      window.requestAnimationFrame = (callback) => { callbacks.set(++next, callback); return next; };
      window.cancelAnimationFrame = (id) => { callbacks.delete(id); };
      (window as unknown as { __CLOSE_FRAMES__: { flush: () => void; restore: () => void } }).__CLOSE_FRAMES__ = {
        flush() {
          const current = [...callbacks.values()];
          callbacks.clear();
          current.forEach((callback) => callback(performance.now()));
        },
        restore() { window.requestAnimationFrame = original; window.cancelAnimationFrame = originalCancel; },
      };
      // Dispatch directly: the test deliberately holds animation frames, which
      // a browser automation click would itself wait on for actionability.
      document.querySelector<HTMLButtonElement>(".request-details-dialog > header button[aria-label='Close']")!.click();
    });
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(opener).not.toBeFocused();
    const flush = () => page.evaluate(() => (window as unknown as { __CLOSE_FRAMES__: { flush: () => void } }).__CLOSE_FRAMES__.flush());
    await flush();
    await expect(opener).not.toBeFocused();
    const otherControl = page.getByRole("button", { name: "More filters", exact: true });
    if (newFocus) await otherControl.focus();
    await flush();
    await page.evaluate(() => (window as unknown as { __CLOSE_FRAMES__: { restore: () => void } }).__CLOSE_FRAMES__.restore());
    await expect(newFocus ? otherControl : opener).toBeFocused();
    if (!newFocus) await expect(page.getByRole("tooltip")).toHaveCount(0);
  });
}

test("keyboard dismissal does not mount a request-ID tooltip during focus restoration", async ({ page }) => {
  await installTauriMock(page, { locale: "en" });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  const opener = page.locator(".usage-request-table .request-link").first();
  await page.keyboard.press("Tab");
  await opener.focus();
  await page.keyboard.press("Enter");
  await expect(page.getByRole("dialog")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(opener).toBeFocused();
  await expect(page.getByRole("tooltip")).toHaveCount(0);
  await page.keyboard.press("Tab");
  await page.keyboard.press("Shift+Tab");
  await expect(opener).toBeFocused();
  await expect(page.getByRole("tooltip")).toHaveText("req_synthetic_local");
});

for (const locale of ["ru", "en"] as const) {
  test(`one Escape closes request details even while its close-button tooltip is visible: ${locale}`, async ({ page }) => {
    await installTauriMock(page, { locale });
    await page.goto("/");
    await page.getByRole("button", { name: locale === "ru" ? "Использование" : "Usage", exact: true }).click();
    await page.locator(".usage-request-table .request-link").first().click();
    const dialog = page.getByRole("dialog");
    await dialog.getByRole("button", { name: locale === "ru" ? "Закрыть" : "Close", exact: true }).hover();
    await expect(page.getByRole("tooltip")).toHaveText(locale === "ru" ? "Закрыть" : "Close");
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
  });
}
