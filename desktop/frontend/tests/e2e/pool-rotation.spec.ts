import { expect, test } from "../bun-playwright";
import type { OperationalStatus, RuntimeSnapshot } from "../../src/features/relay/api/types";
import { emitTauriEvent, installTauriMock } from "./tauri-mock";

test("rotation modes apply keyboard selection immediately", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Pool rotation settings", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Pool rotation", exact: true });
  const automatic = dialog.getByRole("radio", { name: "Automatic", exact: true });
  const expiry = dialog.getByRole("radio", { name: "Expiring first", exact: true });
  const manual = dialog.getByRole("radio", { name: "Round robin", exact: true });
  const reserve = dialog.getByRole("radio", { name: "Primary & reserve", exact: true });
  await automatic.focus();
  await page.keyboard.press("ArrowRight");
  await expect(expiry).toBeFocused();
  await expect(expiry).toHaveAttribute("aria-checked", "true");
  await page.keyboard.press("ArrowDown");
  await expect(manual).toBeFocused();
  await expect(manual).toHaveAttribute("aria-checked", "true");
  await expect(dialog.getByLabel("Request share: Example compatible API", { exact: true })).toHaveCount(0);
  await page.keyboard.press("End");
  await expect(reserve).toBeFocused();
  await expect(reserve).toHaveAttribute("aria-checked", "true");
  await page.keyboard.press("ArrowRight");
  await expect(automatic).toBeFocused();
  await page.keyboard.press("ArrowLeft");
  await expect(reserve).toBeFocused();
  await page.keyboard.press("Home");
  await expect(automatic).toBeFocused();
  await dialog.getByRole("button", { name: "Close", exact: true }).last().click();
  await expect(dialog).toBeHidden();
  const saves = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "update_local_routing").length);
  expect(saves).toBeGreaterThan(0);
  await page.getByRole("button", { name: "Pool rotation settings", exact: true }).click();
  await expect(automatic).toHaveAttribute("aria-checked", "true");
});

for (const [width, height] of [[1160, 844], [1160, 540], [820, 844], [600, 844], [390, 844], [360, 640]]) {
  for (const theme of ["light", "dark"] as const) {
    test(`mixed rotation fits ${theme} at ${width}x${height}`, async ({ page }, testInfo) => {
      await page.setViewportSize({ width, height });
      await installTauriMock(page, { mode: "local", locale: "ru", populated: true, theme, sourceCount: 3, accountCount: 5 });
      await page.goto("/");
      await page.getByRole("button", { name: "Пул", exact: true }).click();
      await page.getByRole("button", { name: "Настройки ротации пула", exact: true }).click();
      const dialog = page.getByRole("dialog", { name: "Ротация пула", exact: true });
      await expect(dialog.getByRole("listitem")).toHaveCount(8);
      await expect(dialog.getByRole("radio")).toHaveCount(4);
      await expect(dialog.locator("aside")).toHaveCount(1);
      const layout = await dialog.evaluate((element) => {
        const rect = element.getBoundingClientRect();
        const strategy = element.querySelector<HTMLElement>(".pool-routing-strategy")!.getBoundingClientRect();
        const content = element.querySelector<HTMLElement>(".pool-routing-content")!;
        const contentRect = content.getBoundingClientRect();
        const overflow = [...element.querySelectorAll<HTMLElement>(".pool-routing-member, .pool-routing-modes button, .pool-routing-content")]
          .filter((item) => item.scrollWidth > item.clientWidth + 1)
          .map((item) => ({ className: item.className, clientWidth: item.clientWidth, scrollWidth: item.scrollWidth }));
        return {
          contained: rect.left >= 0 && rect.right <= innerWidth && rect.top >= 0 && rect.bottom <= innerHeight,
          strategyPlacement: innerWidth <= 600 ? strategy.bottom <= contentRect.top + 1 : strategy.right <= contentRect.left + 1,
          noDialogOverflow: element.scrollWidth <= element.clientWidth + 1,
          overflow,
        };
      });
      expect(layout, JSON.stringify(layout)).toEqual({ contained: true, strategyPlacement: true, noDialogOverflow: true, overflow: [] });
      await page.screenshot({ path: testInfo.outputPath("rotation.png"), animations: "disabled" });
      await expect(dialog.getByRole("listitem").first()).toHaveAttribute("data-status", "rotation");
      await dialog.getByRole("listitem").last().scrollIntoViewIfNeeded();
      await expect(dialog.getByRole("listitem").last().getByRole("spinbutton").last()).toBeVisible();
      await expect(dialog.getByText("Восстановление после ошибок", { exact: true })).toHaveCount(0);
      await expect(dialog.getByRole("checkbox")).toHaveCount(0);
      await expect(dialog.getByRole("button", { name: "Закрыть", exact: true }).last()).toBeInViewport();
      const scrollContainers = await dialog.evaluate((element) => [...element.querySelectorAll<HTMLElement>("*")]
        .filter((item) => /^(auto|scroll)$/.test(getComputedStyle(item).overflowY) && item.scrollHeight > item.clientHeight + 1)
        .map((item) => item.className));
      expect(scrollContainers.length).toBeLessThanOrEqual(1);
      expect(scrollContainers.every((name) => name === (width <= 600 ? "pool-routing-editor" : "pool-routing-content"))).toBe(true);
      if (width <= 600) expect(scrollContainers).toEqual(["pool-routing-editor"]);
      await dialog.getByRole("radio", { name: "По кругу", exact: true }).click();
      expect(await dialog.getByRole("listitem").evaluateAll((items) => items.every((item) => item.scrollWidth <= item.clientWidth + 1))).toBe(true);
      await page.screenshot({ path: testInfo.outputPath("rotation-manual.png"), animations: "disabled" });
      await dialog.getByRole("checkbox", { name: "Свои доли запросов", exact: true }).check();
      expect(await dialog.locator(".pool-routing-member, .pool-routing-columns").evaluateAll((items) => items.every((item) => item.scrollWidth <= item.clientWidth + 1))).toBe(true);
      await dialog.getByRole("listitem").last().scrollIntoViewIfNeeded();
      await expect(dialog.getByRole("listitem").last().getByRole("spinbutton").first()).toBeVisible();
      await expect(dialog.getByRole("button", { name: "Закрыть", exact: true }).last()).toBeInViewport();
    });
  }
}

for (const theme of ["light", "dark"] as const) {
  test(`desktop rotation keeps a dense single-line list in ${theme}`, async ({ page }, testInfo) => {
    const longName = "Synthetic long account identifier with deliberately extended display text for compact routing and unchanged row height@example.test";
    await page.setViewportSize({ width: 1160, height: 844 });
    await installTauriMock(page, {
      mode: "local", locale: "ru", populated: true, theme, sourceCount: 1,
      accountRouteKinds: Array.from({ length: 21 }, () => "codex" as const),
    });
    await page.addInitScript((longName) => {
      const native = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
      const invoke = native.invoke.bind(native);
      native.invoke = async (command, args, options) => {
        const result = await invoke(command, args, options);
        if (command === "get_local_runtime_state") {
          (result as RuntimeSnapshot).accounts.forEach((account, index) => {
            account.label = index === 0 ? longName : `Synthetic account ${index + 1}`;
            account.identityHint = account.label;
          });
        }
        return result;
      };
    }, longName);
    await page.goto("/");
    await page.getByRole("button", { name: "Пул", exact: true }).click();
    await page.getByRole("button", { name: "Настройки ротации пула", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "Ротация пула", exact: true });
    await expect(dialog.getByRole("radiogroup", { name: "Режим ротации", exact: true })).toBeVisible();
    await expect(dialog.getByRole("listitem")).toHaveCount(22);
    await expect(dialog.locator(".pool-routing-meta, .pool-routing-name small")).toHaveCount(0);
    const accountIcon = dialog.locator('[data-member-id^="account:"] .pool-routing-avatar').first();
    const longLabel = dialog.locator('[data-member-id="account:account_synthetic_1"] .pool-routing-name strong');
    await expect(longLabel).toHaveText(longName);
    await expect(longLabel).toHaveAttribute("data-relay-tooltip", longName);
    expect(await longLabel.evaluate((label) => label.scrollWidth > label.clientWidth)).toBe(true);
    await expect(accountIcon).toHaveAccessibleDescription("Аккаунт");
    await expect(accountIcon).toHaveAttribute("data-relay-tooltip", /^Аккаунт · /);
    for (const name of ["Автоматически", "По кругу"]) {
      await dialog.getByRole("radio", { name, exact: true }).click();
      const layout = await dialog.evaluate((element) => ({
        rows: [...element.querySelectorAll<HTMLElement>(".pool-routing-member")].map((row) => row.getBoundingClientRect().height),
        modes: [...element.querySelectorAll<HTMLElement>(".pool-routing-modes button")].map((button) => button.getBoundingClientRect().height),
        wrappingNames: [...element.querySelectorAll<HTMLElement>(".pool-routing-name strong")].filter((label) => getComputedStyle(label).whiteSpace !== "nowrap").length,
      }));
      expect(layout.rows.every((height) => height >= 40 && height <= 46), JSON.stringify(layout)).toBe(true);
      expect(layout.modes.every((height) => height <= 38), JSON.stringify(layout)).toBe(true);
      expect(layout.wrappingNames).toBe(0);
    }
    await page.mouse.move(0, 0);
    await dialog.screenshot({ path: testInfo.outputPath("rotation-compact.png"), animations: "disabled" });
    await dialog.getByRole("listitem").last().scrollIntoViewIfNeeded();
    await expect(dialog.getByRole("listitem").last().getByRole("spinbutton")).toBeVisible();
  });
}

for (const mode of ["local"] as const) {
  test(`${mode} automatic display groups statuses without changing manual order or member settings`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true, accountCount: 4 });
    await page.goto("/");
    const initial = await page.evaluate(async (mode) => {
      const scope = window as unknown as {
        __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
      };
      const invoke = scope.__TAURI_INTERNALS__.invoke.bind(scope.__TAURI_INTERNALS__);
      const snapshotCommand = "get_local_runtime_state";
      scope.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
        const result = await invoke(command, args, options);
        if (command !== snapshotCommand) return result;
        const runtime = result as RuntimeSnapshot;
        const statuses: OperationalStatus[] = ["unavailable", "disabled", "quotaWait", "rotation"];
        runtime.accounts.forEach((account, index) => { account.operationalStatus = statuses[index]; });
        runtime.gateway.poolRotation = { preset: null, memberOrder: [
          `account:${runtime.accounts[3]!.id}`, `source:${runtime.sources[0]!.id}`,
          `account:${runtime.accounts[2]!.id}`, `account:${runtime.accounts[0]!.id}`, `account:${runtime.accounts[1]!.id}`,
        ] };
        return runtime;
      };
      return (await invoke(snapshotCommand) as RuntimeSnapshot).gateway.poolRouting!;
    }, mode);
    await emitTauriEvent(page, "zenith-state-changed", null);
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await page.getByRole("button", { name: "Pool rotation settings", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "Pool rotation", exact: true });
    const rows = dialog.getByRole("listitem");
    for (const name of ["Automatic", "Expiring first"]) {
      await dialog.getByRole("radio", { name, exact: true }).click();
      await expect(dialog.getByRole("button", { name: /^Reorder / })).toHaveCount(0);
      expect(await rows.evaluateAll((items) => items.map((item) => item.getAttribute("data-status"))))
        .toEqual(["rotation", "rotation", "quotaWait", "unavailable", "disabled"]);
      for (const label of ["In rotation", "Waiting for quota", "Unavailable", "Disabled"]) {
        await expect(dialog.getByRole("img", { name: label, exact: true }).first()).toBeVisible();
        await expect(dialog.getByText(label, { exact: true })).toHaveCount(0);
      }
    }
    const edited = dialog.locator('[data-member-id="account:account_synthetic_4"]');
    await expect(edited.getByRole("spinbutton").last()).toHaveValue("");
    await expect(edited.getByRole("spinbutton").last()).toHaveAttribute("placeholder", "Unlimited");
    await edited.getByRole("spinbutton").last().press("ArrowUp");
    await expect(edited.getByRole("spinbutton").last()).toHaveValue("1");
    await edited.getByRole("spinbutton").first().fill("7");
    await edited.getByRole("spinbutton").last().fill("2");
    await dialog.getByRole("radio", { name: "Round robin", exact: true }).click();
    expect(await rows.evaluateAll((items) => items.map((item) => item.getAttribute("data-member-id"))))
      .toEqual(initial.members.map((member) => `${member.kind}:${member.id}`));
    await dialog.getByRole("radio", { name: "Automatic", exact: true }).click();
    await dialog.getByRole("button", { name: "Close", exact: true }).last().click();
    await expect(dialog).toBeHidden();
    await page.getByRole("button", { name: "Pool rotation settings", exact: true }).click();
    await expect(edited.getByRole("spinbutton").first()).toHaveValue("7");
    await expect(edited.getByRole("spinbutton").last()).toHaveValue("2");
    await dialog.getByRole("radio", { name: "Round robin", exact: true }).click();
    expect(await rows.evaluateAll((items) => items.map((item) => item.getAttribute("data-member-id"))))
      .toEqual(initial.members.map((member) => `${member.kind}:${member.id}`));
    await edited.getByRole("spinbutton").fill("");
    await expect(edited.getByRole("spinbutton")).toHaveAttribute("aria-valuetext", "Unlimited");
    await dialog.getByRole("button", { name: "Close", exact: true }).last().click();
    await expect(dialog).toBeHidden();
    const savedLimit = await page.evaluate(async (mode) => {
      const invoke = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string) => Promise<RuntimeSnapshot> } }).__TAURI_INTERNALS__.invoke;
      const runtime = await invoke("get_local_runtime_state");
      return runtime.gateway.poolRouting?.members.find((member) => member.kind === "account" && member.id === "account_synthetic_4")?.maxConcurrency;
    }, mode);
    expect(savedLimit).toBe(0);
    await page.getByRole("button", { name: "Pool rotation settings", exact: true }).click();
    await expect(edited.getByRole("spinbutton")).toHaveValue("");
    await expect(edited.getByRole("spinbutton")).toHaveAttribute("placeholder", "Unlimited");
  });
}

test("rotation refreshes concurrent membership changes without blocking edits", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Pool rotation settings", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Pool rotation", exact: true });
  await dialog.getByRole("radio", { name: "Round robin", exact: true }).click();
  await expect(dialog.locator(".pool-routing-editor")).toHaveAttribute("aria-busy", "false");
  await page.evaluate(async () => {
    const invoke = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__.invoke;
    await invoke("set_local_pool_membership", { input: { accountIds: ["account_synthetic"], sourceIds: [], inPool: false } });
  });
  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect(dialog.locator('[data-member-id="account:account_synthetic"]')).toHaveCount(0);
  await dialog.getByRole("radio", { name: "Automatic", exact: true }).click();
  await expect(dialog.locator(".pool-routing-editor")).toHaveAttribute("aria-busy", "false");
  await expect(dialog.getByRole("alert")).toHaveCount(0);
  await expect(dialog.getByRole("listitem")).toHaveCount(1);
});
