import { expect, test } from "../bun-playwright";
import type { OperationalStatus, RuntimeSnapshot } from "../../src/features/relay/api/types";
import { emitTauriEvent, installTauriMock } from "./tauri-mock";

test("rotation modes apply keyboard selection immediately", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Pool rotation settings", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Pool rotation", exact: true });
  const smart = dialog.getByRole("radio", { name: "Smart", exact: true });
  const inOrder = dialog.getByRole("radio", { name: "In order", exact: true });
  const roundRobin = dialog.getByRole("radio", { name: "Round robin", exact: true });
  await smart.focus();
  await page.keyboard.press("ArrowRight");
  await expect(inOrder).toBeFocused();
  await expect(inOrder).toHaveAttribute("aria-checked", "true");
  await expect(dialog.getByLabel("Request share: Example compatible API", { exact: true })).toHaveCount(0);
  await page.keyboard.press("End");
  await expect(roundRobin).toBeFocused();
  await expect(roundRobin).toHaveAttribute("aria-checked", "true");
  await expect(dialog.getByLabel("Request share: Example compatible API", { exact: true })).toBeEnabled();
  await page.keyboard.press("ArrowRight");
  await expect(smart).toBeFocused();
  await page.keyboard.press("ArrowLeft");
  await expect(roundRobin).toBeFocused();
  await page.keyboard.press("Home");
  await expect(smart).toBeFocused();
  await dialog.getByRole("button", { name: "Close", exact: true }).last().click();
  await expect(dialog).toBeHidden();
  const saves = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "update_local_routing").length);
  expect(saves).toBeGreaterThan(0);
  await page.getByRole("button", { name: "Pool rotation settings", exact: true }).click();
  await expect(smart).toHaveAttribute("aria-checked", "true");
});

for (const [width, height] of [[1160, 844], [1160, 540], [600, 844], [390, 844], [360, 640]]) {
  for (const theme of ["light", "dark"] as const) {
    test(`mixed rotation fits ${theme} at ${width}x${height}`, async ({ page }, testInfo) => {
      await page.setViewportSize({ width, height });
      await installTauriMock(page, { mode: "local", locale: "ru", populated: true, theme, sourceCount: 3, accountCount: 5 });
      await page.goto("/");
      await page.getByRole("button", { name: "Пул", exact: true }).click();
      await page.getByRole("button", { name: "Настройки ротации пула", exact: true }).click();
      const dialog = page.getByRole("dialog", { name: "Ротация пула", exact: true });
      await expect(dialog.getByRole("listitem")).toHaveCount(8);
      await expect(dialog.getByRole("radio")).toHaveCount(3);
      expect(await dialog.evaluate((element) => {
        const rect = element.getBoundingClientRect();
        return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 0 && rect.bottom <= innerHeight
          && element.scrollWidth <= element.clientWidth + 1
          && [...element.querySelectorAll<HTMLElement>(".pool-routing-member, .pool-routing-modes button")].every((item) => item.scrollWidth <= item.clientWidth + 1);
      })).toBe(true);
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
      expect(scrollContainers.every((name) => name === "relay-dialog-body")).toBe(true);
      if (width <= 600 || height <= 540) expect(scrollContainers).toEqual(["relay-dialog-body"]);
      await dialog.getByRole("radio", { name: "По порядку", exact: true }).click();
      expect(await dialog.getByRole("listitem").evaluateAll((items) => items.every((item) => item.scrollWidth <= item.clientWidth + 1))).toBe(true);
      await page.screenshot({ path: testInfo.outputPath("rotation-manual.png"), animations: "disabled" });
    });
  }
}

for (const mode of ["local", "remote"] as const) {
  test(`${mode} automatic display groups statuses without changing manual order or member settings`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true, accountCount: 4 });
    await page.goto("/");
    const initial = await page.evaluate(async (mode) => {
      const scope = window as unknown as {
        __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
      };
      const invoke = scope.__TAURI_INTERNALS__.invoke.bind(scope.__TAURI_INTERNALS__);
      const snapshotCommand = mode === "local" ? "get_local_runtime_state" : "get_remote_server_state";
      scope.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
        const result = await invoke(command, args, options);
        if (command !== snapshotCommand) return result;
        const runtime = result as RuntimeSnapshot;
        const statuses: OperationalStatus[] = ["unavailable", "disabled", "quotaWait", "rotation"];
        runtime.accounts.forEach((account, index) => { account.operationalStatus = statuses[index]; });
        return runtime;
      };
      return (await invoke(snapshotCommand) as RuntimeSnapshot).gateway.poolRouting!;
    }, mode);
    await emitTauriEvent(page, "zenith-state-changed", null);
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await page.getByRole("button", { name: "Pool rotation settings", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "Pool rotation", exact: true });
    const rows = dialog.getByRole("listitem");
    for (const name of ["Smart", "Round robin"]) {
      await dialog.getByRole("radio", { name, exact: true }).click();
      await expect(dialog.getByRole("button", { name: /^Reorder / })).toHaveCount(0);
      expect(await rows.evaluateAll((items) => items.map((item) => item.getAttribute("data-status"))))
        .toEqual(["rotation", "rotation", "quotaWait", "unavailable", "disabled"]);
      for (const label of ["In rotation", "Waiting for quota", "Unavailable", "Disabled"]) {
        await expect(dialog.getByText(label, { exact: true }).first()).toBeVisible();
      }
    }
    const edited = dialog.locator('[data-member-id="account:account_synthetic_4"]');
    await expect(edited.getByRole("spinbutton").last()).toHaveValue("");
    await expect(edited.getByRole("spinbutton").last()).toHaveAttribute("placeholder", "Unlimited");
    await edited.getByRole("spinbutton").last().press("ArrowUp");
    await expect(edited.getByRole("spinbutton").last()).toHaveValue("1");
    await edited.getByRole("spinbutton").first().fill("7");
    await edited.getByRole("spinbutton").last().fill("2");
    await dialog.getByRole("radio", { name: "In order", exact: true }).click();
    expect(await rows.evaluateAll((items) => items.map((item) => item.getAttribute("data-member-id"))))
      .toEqual(initial.members.map((member) => `${member.kind}:${member.id}`));
    await dialog.getByRole("radio", { name: "Smart", exact: true }).click();
    await dialog.getByRole("button", { name: "Close", exact: true }).last().click();
    await expect(dialog).toBeHidden();
    await page.getByRole("button", { name: "Pool rotation settings", exact: true }).click();
    await expect(edited.getByRole("spinbutton").first()).toHaveValue("7");
    await expect(edited.getByRole("spinbutton").last()).toHaveValue("2");
    await dialog.getByRole("radio", { name: "In order", exact: true }).click();
    expect(await rows.evaluateAll((items) => items.map((item) => item.getAttribute("data-member-id"))))
      .toEqual(initial.members.map((member) => `${member.kind}:${member.id}`));
    await edited.getByRole("spinbutton").fill("");
    await expect(edited.getByRole("spinbutton")).toHaveAttribute("aria-valuetext", "Unlimited");
    await dialog.getByRole("button", { name: "Close", exact: true }).last().click();
    await expect(dialog).toBeHidden();
    const savedLimit = await page.evaluate(async (mode) => {
      const invoke = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string) => Promise<RuntimeSnapshot> } }).__TAURI_INTERNALS__.invoke;
      const runtime = await invoke(mode === "local" ? "get_local_runtime_state" : "get_remote_server_state");
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
  await dialog.getByRole("radio", { name: "In order", exact: true }).click();
  await expect(dialog.locator(".pool-routing-editor")).toHaveAttribute("aria-busy", "false");
  await page.evaluate(async () => {
    const invoke = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__.invoke;
    await invoke("set_local_pool_membership", { input: { accountIds: ["account_synthetic"], sourceIds: [], inPool: false } });
  });
  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect(dialog.locator('[data-member-id="account:account_synthetic"]')).toHaveCount(0);
  await dialog.getByRole("radio", { name: "Round robin", exact: true }).click();
  await expect(dialog.locator(".pool-routing-editor")).toHaveAttribute("aria-busy", "false");
  await expect(dialog.getByRole("alert")).toHaveCount(0);
  await expect(dialog.getByRole("listitem")).toHaveCount(1);
});
