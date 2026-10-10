import { expect, test } from "../../bun-playwright";
import { installTauriMock } from "../tauri-mock";
import { chooseOption } from "./helpers";

test("automation editor only saves executable local configurations", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3, codexBindings: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Automations" }).click();
  await page.getByRole("button", { name: "Edit" }).click();

  const dialog = page.getByRole("dialog", { name: "Edit automation" });
  const save = dialog.getByRole("button", { name: "Save" });
  await dialog.getByRole("button", { name: /^Accounts:/ }).click();
  await expect(page.locator('[role="option"][data-value="tags"]')).toHaveCount(0);
  await page.locator('[role="option"][data-value="account_ids"]').click();
  await expect(save).toBeDisabled();

  await dialog.getByLabel("Personal Plus").check();
  await dialog.getByLabel("Backup account").check();
  await dialog.getByRole("button", { name: /^Model:/ }).click();
  await expect(page.locator('[role="option"][data-value="gpt-5.4-mini"]')).toHaveCount(1);
  await expect(page.locator('[role="option"][data-value="gpt-5.4"]')).toHaveCount(0);
  await expect(page.locator('[role="option"][data-value="o3"]')).toHaveCount(0);
  await page.locator('[role="option"][data-value="gpt-5.4-mini"]').click();

  await expect(dialog.getByRole("button", { name: "Manual" })).toHaveCount(0);
  await save.click();

  const call = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: Record<string, unknown> } }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "update_quota_wake_automation"));
  expect(call?.args.input).toMatchObject({
    accountSelector: { kind: "account_ids", values: ["account_synthetic", "account_synthetic_3"] },
    windowKinds: ["primary"],
    modelPolicy: { kind: "explicit", value: "gpt-5.4-mini" },
    executionPolicy: "automatic",
  });
});

test("weekly automation is automatic and targets the secondary quota window", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Automations" }).click();
  await page.getByRole("button", { name: "Edit" }).click();

  const dialog = page.getByRole("dialog", { name: "Edit automation" });
  await chooseOption(page, dialog, "Automation type", "weekly");
  await expect(dialog.getByLabel("Name", { exact: true })).toHaveValue("");
  await expect(dialog.getByText("When weekly quota is exhausted", { exact: true })).toBeVisible();
  await expect(dialog.getByText("Automatic", { exact: true })).toHaveCount(0);
  await expect(dialog.getByRole("group", { name: "Run" })).toHaveCount(0);
  await expect(dialog.getByRole("button", { name: /^Model:/ })).toHaveCount(0);
  await dialog.getByRole("button", { name: "Save", exact: true }).click();

  const call = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: Record<string, unknown> } }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "update_quota_wake_automation"));
  expect(call?.args.input).toMatchObject({
    name: "Reset weekly quota",
    trigger: { kind: "weekly" },
    windowKinds: ["secondary"],
    modelPolicy: { kind: "lightest_supported" },
    executionPolicy: "automatic",
  });
  await expect(page.locator(".automation-card header strong")).toHaveText("Reset weekly quota");
});

test("automation type controls its fields and default name while custom names are optional", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Automations" }).click();
  await page.getByRole("button", { name: "Add automation", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Add automation" });
  const name = dialog.getByLabel("Name", { exact: true });
  await expect(name).toHaveValue("");
  await expect(dialog.getByRole("button", { name: "Automation type: Start quota countdown", exact: true })).toBeVisible();
  await chooseOption(page, dialog, "Automation type", "weekly");
  await expect(dialog.getByRole("button", { name: /^Model:/ })).toHaveCount(0);
  await expect(name).toHaveValue("");
  await chooseOption(page, dialog, "Automation type", "quota_full");
  await expect(dialog.getByRole("button", { name: /^Model:/ })).toBeVisible();
  await name.fill("Work accounts");
  await chooseOption(page, dialog, "Automation type", "weekly");
  await expect(name).toHaveValue("Work accounts");
  await dialog.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.locator(".automation-card header strong")).toHaveText("Work accounts");
  await expect(page.locator(".automation-card header .connection-identity small")).toHaveText("Reset weekly quota");
  await expect(page.getByPlaceholder("Search", { exact: true })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Refresh", exact: true })).toHaveCount(0);
  const enabled = page.locator(".automation-card").getByRole("checkbox", { name: "Enabled", exact: true });
  await enabled.uncheck();
  await expect(enabled).not.toBeChecked();
  await page.getByRole("button", { name: "Edit", exact: true }).click();
  const editor = page.getByRole("dialog", { name: "Edit automation" });
  await chooseOption(page, editor, "Automation type", "quota_full");
  await expect(editor.getByLabel("Name", { exact: true })).toHaveValue("Work accounts");
  await editor.getByLabel("Name", { exact: true }).fill("   ");
  await editor.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.locator(".automation-card header strong")).toHaveText("Start quota countdown");
});

for (const mode of ["local", "remote"] as const) {
  test(`automation cards and editor stay consistent without a toolbar in ${mode} mode`, async ({ page }) => {
    await installTauriMock(page, {
      mode, locale: "ru", populated: true,
      automation: { name: "Start quota countdown", trigger: { kind: "weekly" }, windowKinds: ["secondary"] },
    });
    await page.goto("/");
    await page.getByRole("button", { name: "Подключения", exact: true }).click();
    await page.getByRole("tab", { name: "Источники API", exact: true }).click();
    await page.getByPlaceholder("Поиск", { exact: true }).fill("no matching source");
    await page.getByRole("tab", { name: "Автоматизация", exact: true }).click();
    await expect(page.locator(".automation-card header strong")).toHaveText("Сбросить недельную квоту");
    await expect(page.locator(".automation-card")).toContainText("При исчерпании недельной квоты");
    await expect(page.getByPlaceholder("Поиск", { exact: true })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Обновить", exact: true })).toHaveCount(0);
    await expect(page.locator(".automation-card")).toHaveCount(1);
    await page.getByRole("button", { name: "Изменить", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "Изменить автоматизацию" });
    await expect(dialog.getByLabel("Название", { exact: true })).toHaveValue("");
    await expect(dialog.getByRole("button", { name: "Тип автоматизации: Сбросить недельную квоту", exact: true })).toBeVisible();
    await dialog.getByRole("button", { name: "Сохранить", exact: true }).click();
    const saved = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { name?: string; payload?: { name?: string } } } }> }).__TAURI_TEST_INVOKES__.findLast((item) => ["update_quota_wake_automation", "execute_remote_server_action"].includes(item.command)));
    expect(saved?.args.input?.payload?.name ?? saved?.args.input?.name).toBe("Сбросить недельную квоту");
  });
}

test("remote automation editor exposes only automatic execution", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Automations" }).click();
  await page.getByRole("button", { name: "Edit" }).click();

  const dialog = page.getByRole("dialog", { name: "Edit automation" });
  await expect(dialog.getByRole("group", { name: "Run" })).toHaveCount(0);
  await expect(dialog.getByText("Automatic", { exact: true })).toHaveCount(0);
  await expect(dialog.getByRole("button", { name: /^Model: gpt-5\.4/ })).toBeVisible();
  await dialog.getByRole("button", { name: /^Accounts:/ }).click();
  await expect(page.locator('[role="option"][data-value="tags"]')).toHaveCount(0);
  await page.keyboard.press("Escape");
  await dialog.getByRole("button", { name: "Save" }).click();

  const call = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { action?: Record<string, unknown>; payload?: Record<string, unknown> } } }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "execute_remote_server_action"));
  expect(call?.args.input).toMatchObject({ action: { type: "update_wake_task", id: "wake_synthetic" }, payload: { executionPolicy: "automatic", modelPolicy: { kind: "explicit", value: "gpt-5.4" } } });
});

for (const mode of ["local", "remote"] as const) {
  test(`API source usage keeps source diagnostics in ${mode} mode`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true, usageCandidateKind: "source" });
    await page.goto("/");
    await page.getByRole("button", { name: "Usage", exact: true }).click();
    await page.getByRole("button", { name: new RegExp(`Request details: req_synthetic_${mode}`) }).click();
    const dialog = page.getByRole("dialog", { name: "Request details" });
    await expect(dialog).toContainText("Example compatible API");
    await dialog.getByRole("tab", { name: "Route", exact: true }).click();
    await expect(dialog).toContainText("Weighted rotation");
    await expect(dialog.getByText("Quota at selection", { exact: true })).toHaveCount(0);
  });

  test(`usage distinguishes the client model from the routed model in ${mode} mode`, async ({ page }) => {
    await installTauriMock(page, {
      mode, locale: "en", populated: true,
      usageRequestedModel: "public-alias", usageResolvedModel: "provider/model",
    });
    await page.goto("/");
    await page.getByRole("button", { name: "Usage", exact: true }).click();
    await page.getByRole("button", { name: new RegExp(`Request details: req_synthetic_${mode}`) }).click();
    const dialog = page.getByRole("dialog", { name: "Request details" });
    await expect(dialog.getByText("Requested model", { exact: true })).toBeVisible();
    await expect(dialog.getByText("Sent to source", { exact: true })).toBeVisible();
    await expect(dialog.getByText("public-alias", { exact: true })).toBeVisible();
    await expect(dialog.locator("dl").getByText("provider/model", { exact: true })).toBeVisible();
  });
}

for (const mode of ["local", "remote"] as const) {
  test(`${mode} keeps transport tied to sign-in and reports it in usage`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true, basisPointsAvailable: true, usageEndpointKind: "excel_basis_points" });
    await page.goto("/");
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await expect(page.getByRole("tab", { name: "Accounts", exact: true })).toHaveAttribute("aria-selected", "true");
    await expect(page.locator(".account-transport-badge")).toHaveCount(0);
    const transport = page.getByRole("checkbox", { name: "Use Basis Points" });
    await expect(transport).toHaveCount(0);

    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await expect(page.locator('.pool-member-card .account-transport-badge')).toHaveCount(0);
    await expect(page.locator(".pool-controls").getByRole("checkbox")).toHaveCount(0);
    await page.getByRole("radio", { name: "Fast", exact: true }).click();
    await expect(page.getByRole("radio", { name: "Fast", exact: true })).toBeEnabled();
    await page.getByRole("button", { name: "API", exact: true }).click();
    await expect(transport).toHaveCount(0);
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await expect(page.getByRole("radio", { name: "Fast", exact: true })).toBeChecked();
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await expect(page.getByRole("tab", { name: "Accounts", exact: true })).toHaveAttribute("aria-selected", "true");
    await expect(transport).toHaveCount(0);
    await page.getByRole("tab", { name: "Sources", exact: true }).click();
    await expect(transport).toHaveCount(0);
    await page.getByRole("button", { name: "API", exact: true }).click();
    await expect(transport).toHaveCount(0);
    const updates = await page.evaluate(() => (window as unknown as {
      __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { basisPointsEnabled?: boolean; payload?: Record<string, unknown> } } }>;
    }).__TAURI_TEST_INVOKES__.flatMap(({ command, args }) => {
      const input = command === "update_local_routing" ? args.input
        : command === "execute_remote_server_action" ? args.input?.payload : undefined;
      return typeof input?.basisPointsEnabled === "boolean" ? [input] : [];
    }));
    expect(updates).toEqual([]);

    await page.getByRole("button", { name: "Usage", exact: true }).click();
    await page.getByRole("button", { name: new RegExp(`Request details: req_synthetic_${mode}`) }).click();
    const details = page.getByRole("dialog", { name: "Request details" });
    await expect(details.getByText("Excel / Basis Points", { exact: true })).toBeVisible();
  });
}

test("API pricing shows cache-write TTL fields for Messages routes without inventing other prices", async ({ page }) => {
  await installTauriMock(page, {
    mode: "local",
    locale: "en",
    populated: true,
    mixedModels: true,
    sourceProtocolBindings: [
      { wireApi: "responses", adapter: "native", reasoningMode: "disabled", modelIds: ["gpt-5.4", "gemini-3.1-pro-preview", "grok-4.5", "glm-5.2", "private-model"] },
      { wireApi: "messages", adapter: "native", reasoningMode: "disabled", modelIds: ["claude-opus-4-8"] },
    ],
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources" }).click();
  await page.locator(".source-card").filter({ hasText: "Example compatible API" }).getByRole("button", { name: "Edit" }).click();
  const dialog = page.getByRole("dialog", { name: "Edit source" });
  await dialog.getByRole("tab", { name: "Pricing" }).click();
  await expect(dialog.locator(".source-price-group > summary")).toHaveText([
    "OpenAIModels: 1",
    "AnthropicModels: 1",
    "GoogleModels: 1",
    "xAIModels: 1",
    "Z.aiModels: 1",
    "OtherModels: 1",
  ]);

  await dialog.locator(".source-price-group > summary").filter({ hasText: "OpenAI" }).click();
  await expect(dialog.getByRole("textbox", { name: /cache write price for gpt-5.4/i })).toHaveCount(0);
  await dialog.locator(".source-price-group > summary").filter({ hasText: "Anthropic" }).click();
  await dialog.getByRole("textbox", { name: "Input token price for claude-opus-4-8", exact: true }).fill("1.4");
  await dialog.getByRole("textbox", { name: "Output token price for claude-opus-4-8", exact: true }).fill("7");
  await dialog.getByRole("textbox", { name: "Cached input token price for claude-opus-4-8", exact: true }).fill("1.6");
  await dialog.getByRole("textbox", { name: "5-minute cache write price for claude-opus-4-8" }).fill("2.1");
  await dialog.getByRole("textbox", { name: "1-hour cache write price for claude-opus-4-8" }).fill("4.2");
  await dialog.getByRole("button", { name: "Save" }).click();

  await expect.poll(() => page.evaluate(() => {
    const call = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { modelPriceOverrides?: Record<string, unknown> } } }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "update_local_source");
    return call?.args.input?.modelPriceOverrides?.["claude-opus-4-8"];
  })).toEqual({ inputMicroUsdPerMillion: 1_400_000, outputMicroUsdPerMillion: 7_000_000, cachedInputMicroUsdPerMillion: 1_600_000, cacheWrite5mMicroUsdPerMillion: 2_100_000, cacheWrite1hMicroUsdPerMillion: 4_200_000 });
});

test("explicit cache-write prices remain visible without a Messages route", async ({ page }) => {
  await installTauriMock(page, {
    mode: "local",
    locale: "en",
    populated: true,
    serverModelOrder: ["gpt-5.4", "claude-opus-4-8", "claude-no-cache"],
    modelMetadata: {
      "claude-no-cache": { catalogProvider: "anthropic", catalogFamily: "claude", catalogName: "Claude no cache" },
    },
    sourceProtocolBindings: [{ wireApi: "responses", adapter: "native", reasoningMode: "disabled", modelIds: ["gpt-5.4", "claude-opus-4-8", "claude-no-cache"] }],
    sourceDetectedModelPrices: {
      "claude-opus-4-8": {
        inputMicroUsdPerMillion: 1_400_000,
        outputMicroUsdPerMillion: 7_000_000,
        cacheWrite5mMicroUsdPerMillion: 2_100_000,
        cacheWrite1hMicroUsdPerMillion: 4_200_000,
      },
    },
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources" }).click();
  await page.locator(".source-card").filter({ hasText: "Example compatible API" }).getByRole("button", { name: "Edit" }).click();
  const dialog = page.getByRole("dialog", { name: "Edit source" });
  await dialog.getByRole("tab", { name: "Pricing" }).click();
  await dialog.locator(".source-price-group > summary").filter({ hasText: "Anthropic" }).click();
  const group = dialog.locator(".source-price-group").filter({ hasText: "Anthropic" });
  await expect(group.getByRole("textbox", { name: "5-minute cache write price for claude-opus-4-8" })).toHaveAttribute("placeholder", "2.1");
  await expect(group.getByRole("textbox", { name: "1-hour cache write price for claude-opus-4-8" })).toHaveAttribute("placeholder", "4.2");
  await expect(group.getByRole("textbox", { name: "Cached input token price for claude-opus-4-8" })).toHaveAttribute("placeholder", "—");
  await expect(group.getByRole("textbox", { name: /cache write price for claude-no-cache/i })).toHaveCount(0);
  await expect(group.locator(".source-price-row").filter({ hasText: "claude-no-cache" }).locator(".source-price-empty")).toHaveCount(2);
  await dialog.screenshot({ path: "output/playwright/source-cache-write-prices-light.png" });
  await page.setViewportSize({ width: 390, height: 844 });
  expect(await dialog.locator(".relay-dialog-body").evaluate((body) => body.scrollWidth <= body.clientWidth)).toBe(true);
});

test("OpenAI cache-write price is one 30-minute field", async ({ page }) => {
  await installTauriMock(page, {
    mode: "local",
    locale: "en",
    populated: true,
    serverModelOrder: ["gpt-5.4", "gpt-5.6-luna", "claude-opus-4-8"],
    modelMetadata: {
      "gpt-5.6-luna": { catalogProvider: "openai", catalogFamily: "gpt", catalogName: "GPT-5.6 Luna" },
    },
    sourceDetectedModelPrices: {
      "gpt-5.6-luna": {
        inputMicroUsdPerMillion: 200_000,
        outputMicroUsdPerMillion: 1_200_000,
        cachedInputMicroUsdPerMillion: 20_000,
        cacheWrite5mMicroUsdPerMillion: 250_000,
      },
    },
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources" }).click();
  await page.locator(".source-card").filter({ hasText: "Example compatible API" }).getByRole("button", { name: "Edit" }).click();
  const dialog = page.getByRole("dialog", { name: "Edit source" });
  await dialog.getByRole("tab", { name: "Pricing" }).click();

  const openai = dialog.locator(".source-price-group").filter({ hasText: "OpenAI" });
  await openai.locator("summary").click();
  await expect(openai.locator(".member-price-grid-head")).toContainText("Cache write 30 min");
  await expect(openai.locator(".member-price-grid-head")).not.toContainText("5 min");
  await expect(openai.locator(".member-price-grid-head")).not.toContainText("1 hr");
  await expect(openai.getByRole("textbox", { name: "30-minute cache write price for gpt-5.6-luna" })).toHaveAttribute("placeholder", "0.25");
  await expect(openai.getByRole("textbox", { name: /cache write price for gpt-5.4/i })).toHaveCount(0);
  await expect(openai.locator(".source-price-row").filter({ hasText: "gpt-5.4" }).locator(".source-price-empty")).toHaveCount(1);

  const anthropic = dialog.locator(".source-price-group").filter({ hasText: "Anthropic" });
  await anthropic.locator("summary").click();
  await expect(anthropic.getByRole("textbox", { name: "5-minute cache write price for claude-opus-4-8" })).toHaveCount(1);
  await expect(anthropic.getByRole("textbox", { name: "1-hour cache write price for claude-opus-4-8" })).toHaveCount(1);
  await expect(anthropic.getByRole("textbox", { name: /30-minute cache write price/i })).toHaveCount(0);
});

test("API-reported source prices are hints, not manual overrides", async ({ page }) => {
  await installTauriMock(page, {
    mode: "local",
    locale: "en",
    populated: true,
    sourceDetectedModelPrices: {
      "gpt-5.4": {
        inputMicroUsdPerMillion: 2_500_000,
        cachedInputMicroUsdPerMillion: 250_000,
        outputMicroUsdPerMillion: 15_000_000,
      },
    },
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources" }).click();
  await page.locator(".source-card").filter({ hasText: "Example compatible API" }).getByRole("button", { name: "Edit" }).click();
  const dialog = page.getByRole("dialog", { name: "Edit source" });
  await dialog.getByRole("tab", { name: "Pricing" }).click();
  await dialog.locator(".source-price-group > summary").filter({ hasText: "OpenAI" }).click();

  const input = dialog.getByRole("textbox", { name: "Input token price for gpt-5.4", exact: true });
  const cached = dialog.getByRole("textbox", { name: "Cached input token price for gpt-5.4", exact: true });
  const output = dialog.getByRole("textbox", { name: "Output token price for gpt-5.4", exact: true });
  await expect(input).toHaveValue("");
  await expect(input).toHaveAttribute("placeholder", "2.5");
  await expect(cached).toHaveAttribute("placeholder", "0.25");
  await expect(output).toHaveAttribute("placeholder", "15");

  await dialog.getByRole("button", { name: "Save" }).click();
  const sourceUpdate = await page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { modelPriceOverrides?: Record<string, unknown> } } }> }).__TAURI_TEST_INVOKES__;
    return calls.findLast((item) => item.command === "update_local_source")?.args.input?.modelPriceOverrides;
  });
  expect(sourceUpdate).toEqual({});
});

test("pool API source preserves model and price drafts across tabs", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Pool member policy: Example compatible API" }).click();

  const dialog = page.getByRole("dialog", { name: "Pool member policy", exact: true });
  await expect(dialog.locator(".member-model-heading")).toContainText("2 / 2");
  await dialog.getByRole("switch", { name: "Allow gpt-5.4", exact: true }).uncheck();
  await dialog.getByRole("tab", { name: "Pricing", exact: true }).click();
  await dialog.getByRole("textbox", { name: "Input token price for gpt-5.4", exact: true }).fill("1.75");
  await dialog.getByRole("textbox", { name: "Output token price for gpt-5.4", exact: true }).fill("4.5");
  await dialog.getByRole("tab", { name: "Models", exact: true }).click();
  await expect(dialog.locator(".member-model-heading")).toContainText("1 / 2");
  await expect(dialog.getByRole("switch", { name: "Allow gpt-5.4", exact: true })).not.toBeChecked();
  await dialog.getByRole("button", { name: "Save policy" }).click();

  const update = await page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: Record<string, unknown> } }> }).__TAURI_TEST_INVOKES__;
    return calls.findLast((call) => call.command === "update_local_source")?.args.input;
  });
  expect(update).toMatchObject({
    allowedModels: [],
    excludedModels: ["gpt-5.4"],
    modelPriceOverrides: {
      "gpt-5.4": {
        inputMicroUsdPerMillion: 1_750_000,
        outputMicroUsdPerMillion: 4_500_000,
      },
    },
  });
});
