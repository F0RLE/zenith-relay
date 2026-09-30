import { expect, test, type Page } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

type TestMode = "local" | "remote";

async function openPolicy(page: Page, mode: TestMode = "local", remoteFeatures?: string[], locale: "en" | "ru" = "en") {
  await installTauriMock(page, { mode, remoteFeatures, locale, populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "API", exact: true }).click();
  return page.locator(".gateway-tool-policy");
}

async function policyCalls(page: Page, mode: TestMode) {
  return page.evaluate((selectedMode) => {
    const invokes = (window as unknown as {
      __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }>;
    }).__TAURI_TEST_INVOKES__;
    return invokes.filter(({ command, args }) => selectedMode === "local"
      ? command === "set_local_tool_policy"
      : command === "execute_remote_server_action"
        && (args.input as { action?: { type?: string } })?.action?.type === "set_tool_policy");
  }, mode);
}

for (const mode of ["local", "remote"] as const) {
  test(`${mode} tool optimization switch saves its mode immediately`, async ({ page }) => {
    const control = await openPolicy(page, mode);
    const toggle = control.getByRole("checkbox", { name: "Optimized mode", exact: true });

    await expect(control.locator("input")).toHaveCount(1);
    await expect(control.locator("button, textarea, select, input:not([type=checkbox])")).toHaveCount(0);
    await expect(toggle).not.toBeChecked();

    await toggle.setChecked(true);
    await expect.poll(() => policyCalls(page, mode)).toHaveLength(1);
    await expect(page.locator(".global-feedback.success")).toBeVisible();

    const firstCalls = await policyCalls(page, mode);
    const first = firstCalls[0]!.args.input as Record<string, unknown>;
    const firstUpdate = mode === "local" ? first : first.payload as Record<string, unknown>;
    expect(firstUpdate).toEqual({
      policy: { mode: "automatic" },
      expectedPolicy: { mode: "pass_through" },
    });

    await toggle.setChecked(false);
    await expect.poll(() => policyCalls(page, mode)).toHaveLength(2);
    await expect(page.locator(".global-feedback.success")).toBeVisible();
    const calls = await policyCalls(page, mode);
    const second = calls[1]!.args.input as Record<string, unknown>;
    const secondUpdate = mode === "local" ? second : second.payload as Record<string, unknown>;
    expect(secondUpdate).toEqual({
      policy: { mode: "pass_through" },
      expectedPolicy: { mode: "automatic" },
    });

    // Returning to the API settings reads the persisted mode from the snapshot.
    await page.getByRole("button", { name: "Usage", exact: true }).click();
    await page.getByRole("button", { name: "API", exact: true }).click();
    await expect(page.locator(".gateway-tool-policy").getByRole("checkbox", { name: "Optimized mode", exact: true })).not.toBeChecked();
  });
}

test("older remote servers do not expose an unsupported tool policy command", async ({ page }) => {
  const control = await openPolicy(page, "remote", ["local_gateway"]);
  await expect(control).toHaveCount(0);
});

test("failed save restores the switch and a later toggle can succeed", async ({ page }) => {
  const control = await openPolicy(page);
  const toggle = control.getByRole("checkbox", { name: "Optimized mode", exact: true });
  await page.evaluate(() => {
    const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (name: string, args?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const original = internals.invoke.bind(internals);
    let fail = true;
    internals.invoke = async (name, args) => {
      if (name === "set_local_tool_policy" && fail) {
        fail = false;
        throw { code: "conflict", message: "Synthetic conflicting policy" };
      }
      return original(name, args);
    };
  });

  // A rejected save can restore the value before setChecked verifies its result.
  await toggle.click();
  await expect(page.locator(".global-feedback.error")).toBeVisible();
  await expect(toggle).toBeEnabled();
  await expect(toggle).not.toBeChecked();

  await toggle.setChecked(true);
  await expect(page.locator(".global-feedback.success")).toBeVisible();
  await expect(toggle).toBeChecked();
});

const localizedCopy = [
  { locale: "en", toggle: "Optimized mode", title: "Tool optimization", help: "Help" },
  { locale: "ru", toggle: "Оптимизированный режим", title: "Оптимизация инструментов", help: "Помощь" },
] as const;

for (const copy of localizedCopy) {
  test(`tool settings show one short switch: ${copy.locale}`, async ({ page }) => {
    const control = await openPolicy(page, "local", undefined, copy.locale);
    await expect(control.locator("strong")).toHaveText(copy.title);
    await expect(control.locator("input")).toHaveCount(1);
    await expect(control.locator("button, textarea, select, input:not([type=checkbox])")).toHaveCount(0);

    await control.getByRole("checkbox", { name: copy.toggle, exact: true }).setChecked(true);
    await expect(control.getByRole("checkbox", { name: copy.toggle, exact: true })).toBeChecked();
    await expect(page.locator(".global-feedback.success")).toBeVisible();
    await expect(control).not.toContainText("Allowed tools");
    await expect(control).not.toContainText("Excluded tools");
    await expect(control).not.toContainText("Порог");

    await page.getByRole("button", { name: copy.help, exact: true }).click();
    await page.getByRole("navigation", { name: copy.locale === "ru" ? "Содержание" : "On this page" })
      .getByRole("link", { name: "API", exact: true }).click();
    const article = page.locator(".help-document");
    const heading = article.getByRole("heading", { name: copy.title, exact: true });
    await heading.scrollIntoViewIfNeeded();
    await expect(heading).toBeInViewport();
    await expect(article).toContainText("tool_search");
    await expect(article).toContainText("defer_loading");
    await expect(article).not.toContainText("allowlist");
    await expect(article).not.toContainText("denylist");
  });

  for (const width of [390, 1160]) {
    test(`tool optimization switch fits viewport ${copy.locale} ${width}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 920 });
      const control = await openPolicy(page, "local", undefined, copy.locale);
      await control.getByRole("checkbox", { name: copy.toggle, exact: true }).setChecked(true);
      await expect(control.getByRole("checkbox", { name: copy.toggle, exact: true })).toBeChecked();
      await expect(page.locator(".global-feedback.success")).toBeVisible();
      expect(await control.evaluate((element) => Array.from(element.querySelectorAll("input, strong")).every((node) => {
        const rect = node.getBoundingClientRect();
        return node.scrollWidth <= node.clientWidth + 1 && rect.left >= 0 && rect.right <= innerWidth;
      }))).toBe(true);
      const neededHeight = await control.evaluate((element) => element.scrollHeight + 850);
      await page.setViewportSize({ width, height: neededHeight });
      await page.locator(".relay-content").evaluate((element) => { element.scrollTop = 0; });
      await control.screenshot({ path: `output/playwright/tool-policy-${copy.locale}-${width}.png` });
    });
  }
}
