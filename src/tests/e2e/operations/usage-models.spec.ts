import { expect, test } from "../../bun-playwright";
import { installTauriMock } from "../tauri-mock";
import { chooseOption } from "./helpers";

test("usage filters are named and stay scoped to the request report", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.getByRole("button", { name: /^Status:/ }).click();
  await expect(page.getByRole("option").first()).toHaveText("Any status");
  await page.locator('[role="option"][data-value="all"]').click();
  await page.getByRole("button", { name: "More filters" }).click();
  await page.getByRole("button", { name: /^Protocol:/ }).click();
  await expect(page.getByRole("option").first()).toHaveText("Any protocol");
  await page.locator('[role="option"][data-value="responses"]').click();
  await chooseOption(page, page, "Status", "failed");
  await expect.poll(() => page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { success?: boolean; wireApi?: string; includeEvents?: boolean; includeModels?: boolean; includePoolMembers?: boolean } } }> }).__TAURI_TEST_INVOKES__;
    return calls.findLast((call) => call.command === "get_local_usage_page")?.args.input;
  })).toMatchObject({ success: false, wireApi: "responses", includeEvents: true, includeModels: false, includePoolMembers: false });

  await page.getByRole("tab", { name: "Models" }).click();
  await expect.poll(() => page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { success?: boolean; wireApi?: string; includeEvents?: boolean; includeModels?: boolean; includePoolMembers?: boolean } } }> }).__TAURI_TEST_INVOKES__;
    return calls.findLast((call) => call.command === "get_local_usage_page")?.args.input ?? {};
  })).toMatchObject({ includeEvents: false, includeModels: true, includePoolMembers: false });
});

for (const mode of ["local", "remote"] as const) {
  test(`${mode} clearing usage filters resets the date range`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true });
    await page.goto("/");
    await page.getByRole("button", { name: "Usage", exact: true }).click();
    await chooseOption(page, page, "Range", "weekly");
    const selectedRange = () => page.evaluate((command) => {
      const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { range?: string } } }> }).__TAURI_TEST_INVOKES__;
      return calls.findLast((call) => call.command === command)?.args.input?.range ?? null;
    }, mode === "local" ? "get_local_usage_page" : "get_remote_server_usage");
    await expect.poll(selectedRange).toBe("weekly");
    await page.getByRole("button", { name: "Clear filters", exact: true }).click();
    await expect(page.getByRole("button", { name: "Range: All", exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "Clear filters", exact: true })).toHaveCount(0);
    // The all-period report may already be cached. Refresh checks the reset
    // query without requiring a duplicate request just to display that cache.
    await page.getByRole("button", { name: "Refresh", exact: true }).click();
    await expect.poll(selectedRange).toBeNull();
  });
}

test("account usage keeps API equivalent, payback, and provider quota windows separate", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, quotaAvailable: true, accountCount: 4 });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await chooseOption(page, page, "Pool member", "account_synthetic");

  const accountUsage = page.locator(".usage-account-value");
  await expect(accountUsage).toContainText("Personal Plus");
  await expect(accountUsage).toContainText("API equiv.");
  await expect(accountUsage).toContainText("Purchase cost, USD");
  await expect(accountUsage).toContainText("Payback");
  await expect(accountUsage.locator(".usage-window-table thead th")).toHaveText(["Window", "Remaining", "Reset"]);
  await expect(accountUsage.getByRole("row")).toHaveCount(3);
  await expect.poll(() => page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { sourceOrAccountQuery?: string } } }> }).__TAURI_TEST_INVOKES__;
    return calls.findLast((call) => call.command === "get_local_usage_page")?.args.input?.sourceOrAccountQuery;
  })).toMatch(/^(account_synthetic|a1b2c3d4e5f6)$/);

  await page.getByRole("tab", { name: "Models" }).click();
  await expect(page.locator(".usage-aggregate-table thead th")).toHaveText([
    "Model", "Requests", "Input tokens", "Output tokens", "Cache reads", "API equiv.",
  ]);
  await page.setViewportSize({ width: 840, height: 560 });
  await expect(accountUsage).toBeVisible();
});

test("usage request columns reorder, resize, and open details only from the request id", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();

  const table = page.locator(".usage-request-table");
  const row = table.getByRole("row").filter({ hasText: "req_synthetic_local" });
  const requestCell = row.locator('td[data-column="request"]');
  const requestLink = requestCell.getByRole("button", { name: "Request details: req_synthetic_local" });
  const [cellBounds, linkWidth] = await Promise.all([requestCell.boundingBox(), requestLink.evaluate((element) => element.getBoundingClientRect().width)]);
  expect(cellBounds).not.toBeNull();
  const cellWidth = cellBounds!.width;
  expect(linkWidth).toBeLessThan(cellWidth);
  await requestCell.click({ position: { x: cellWidth - 2, y: cellBounds!.height / 2 } });
  await expect(page.getByRole("dialog", { name: "Request details" })).toHaveCount(0);
  await requestLink.click();
  const details = page.getByRole("dialog", { name: "Request details" });
  await expect(details).toBeVisible();
  await expect(details.getByRole("tab", { name: "Overview", exact: true })).toHaveAttribute("aria-selected", "true");
  await expect(details.getByText("Error origin", { exact: true })).toHaveCount(0);
  await expect(details.getByText("6.7 tok/s", { exact: true })).not.toHaveAttribute("data-tone");
  await page.locator(".relay-modal-backdrop").click({ position: { x: 2, y: 2 } });
  await expect(details).toHaveCount(0);

  expect(await table.locator("th, td").evaluateAll((cells) => cells.every((cell) => getComputedStyle(cell).textAlign === "center"))).toBe(true);
  const statusHeading = table.getByLabel(/^Move the Status column/);
  const timeHeading = table.getByLabel(/^Move the Time column/);
  const [statusBounds, timeBounds] = await Promise.all([statusHeading.boundingBox(), timeHeading.boundingBox()]);
  expect(statusBounds).not.toBeNull();
  expect(timeBounds).not.toBeNull();
  await page.mouse.move(statusBounds!.x + statusBounds!.width / 2, statusBounds!.y + statusBounds!.height / 2);
  await page.mouse.down();
  await page.mouse.move(timeBounds!.x + 3, timeBounds!.y + timeBounds!.height / 2, { steps: 5 });
  await page.mouse.up();
  await expect.poll(() => table.locator("thead th").evaluateAll((headers) => headers.map((header) => header.getAttribute("data-column")))).toEqual(["status", "time", "model", "protocol", "tier", "connection", "timing", "speed", "tokens", "equivalent", "request"]);

  const modelResize = table.getByRole("separator", { name: /^Resize the Model column/ });
  const bounds = await modelResize.boundingBox();
  expect(bounds).not.toBeNull();
  await page.mouse.move(bounds!.x + bounds!.width / 2, bounds!.y + bounds!.height / 2);
  await page.mouse.down();
  await page.mouse.move(bounds!.x + bounds!.width / 2 + 36, bounds!.y + bounds!.height / 2, { steps: 4 });
  await page.mouse.up();
  await expect(table).toHaveAttribute("data-resized", "true");
  await expect.poll(() => page.evaluate(() => JSON.parse(localStorage.getItem("relay.usageRequestTableLayout") ?? "null"))).toMatchObject({ order: ["status", "time", "model", "protocol", "tier", "connection", "timing", "speed", "tokens", "equivalent", "request"], widths: { model: expect.any(Number) } });

  await page.reload();
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator(".usage-request-table thead th").first()).toHaveAttribute("data-column", "status");
  await expect(page.locator(".usage-request-table")).toHaveAttribute("data-resized", "true");
  await page.setViewportSize({ width: 840, height: 560 });
  expect(await page.locator(".usage-request-table").evaluate((element) => element.parentElement!.scrollWidth <= element.parentElement!.clientWidth)).toBe(true);
});

test("usage shows the request protocol and keeps aggregate summaries compact", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();

  await expect(page.locator('.usage-request-table th[data-column="protocol"]')).toHaveText("Protocol");
  await expect(page.locator('.usage-request-table td[data-column="protocol"]')).toHaveText("Responses");

  await expect(page.locator(".usage-overview .usage-metric")).toHaveCount(6);
  await page.getByRole("tab", { name: "Models", exact: true }).click();
  await expect(page.locator(".usage-overview .usage-metric")).toHaveCount(3);
  await page.getByRole("tab", { name: "Errors", exact: true }).click();
  await expect(page.locator(".usage-overview .usage-metric")).toHaveCount(3);
  await expect(page.locator(".usage-overview").getByText("Success", { exact: true })).toHaveCount(0);
  await expect(page.locator(".usage-overview").getByText("Generation speed", { exact: true })).toHaveCount(0);
});

test("usage details warn when forwarded tools yield a text-only response", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, usageToolDiagnostics: "forwarded_text_only" });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.getByRole("button", { name: "Request details: req_synthetic_local" }).click();

  const dialog = page.getByRole("dialog", { name: "Request details" });
  await dialog.getByRole("tab", { name: "Tools", exact: true }).click();
  await expect(dialog.getByText("Client tools", { exact: true })).toBeVisible();
  await expect(dialog.locator(".request-details-list").getByText("3", { exact: true })).toBeVisible();
  await expect(dialog.getByText("Tools sent", { exact: true })).toHaveCount(0);
  await expect(dialog.getByText("Text only", { exact: true })).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Copy request ID" })).toBeVisible();
  await expect(dialog.getByText(/Relay forwarded 3 tool definitions/)).toBeVisible();
});

test("usage details do not blame the upstream when tools were not forwarded", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, usageToolDiagnostics: "dropped_text_only" });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.getByRole("button", { name: "Request details: req_synthetic_local" }).click();

  const dialog = page.getByRole("dialog", { name: "Request details" });
  await dialog.getByRole("tab", { name: "Tools", exact: true }).click();
  await expect(dialog.getByText(/Relay forwarded \d+ tool definitions/)).toHaveCount(0);
});

test("local usage omits the obsolete ChatGPT routing banner", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, codexBindingActive: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();

  await expect(page.getByText("ChatGPT currently uses another provider. New requests will not appear in this local history.")).toHaveCount(0);
});

test("usage attributes API token totals to the selected account", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.locator(".account-card .account-token-speed")).toHaveCount(0);
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect(page.locator('[data-member-label="Personal Plus"] .account-value-strip')).toHaveCount(0);
  await expect(page.locator('[data-member-label="Personal Plus"] .quota-meter').first()).toBeVisible();
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  const summary = page.locator(".usage-metrics");
  await expect(summary.locator(":scope > div")).toHaveCount(6);
  expect(await summary.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
  await page.getByRole("tab", { name: "Pool members" }).click();

  const account = page.getByRole("row").filter({ hasText: "Personal Plus" });
  await expect(account.getByRole("cell")).toHaveText(["Personal Plus", "1", "100%", "In20Out8Cache ↓12Cache ↑4Reason5", "28", "≈$0.0001", "6.7 tok/s", "128 ms / 428 ms"]);
  await expect(account.locator(".usage-token-breakdown span")).toHaveText(["In20", "Out8", "Cache ↓12", "Cache ↑4", "Reason5"]);
  await expect(page.locator(".usage-metrics > .usage-metric")).toHaveCount(3);

  await page.getByRole("tab", { name: "Requests" }).click();
  await expect(page.locator(".usage-metrics")).toContainText("Generation speed6.7 tok/s");
  await expect(page.locator(".usage-metrics")).toContainText("E2E speed18.7 tok/s");
  await page.getByRole("button", { name: "Request details: req_synthetic_local" }).click();
  const details = page.getByRole("dialog", { name: "Request details" });
  await expect(details).toContainText("Generation speed6.7 tok/s");
  await expect(details).toContainText("Total time428 ms");
  await expect(details.locator(".request-details-metrics > div")).toHaveCount(4);
  await expect(details).toContainText("Request cost≈$0.0001");
  await details.getByRole("tab", { name: "Tokens", exact: true }).click();
  await expect(details).toContainText("Input tokens20");
  await expect(details).toContainText("Output tokens8");
  await expect(details).toContainText("Cache reads12");
  await expect(details).toContainText("Cache writes4");
  await expect(details).toContainText("Reasoning tokens5");
  await expect(details).toContainText("Total tokens28");
  await expect(details).toContainText("Request cost≈$0.0001");
  await details.getByRole("tab", { name: "Route", exact: true }).click();
  await expect(details).toContainText("Selection reasonGreatest quota remaining");
  await expect(details).toContainText("Eligible participants4");
  await expect(details).toContainText("Quota at selection63.00%");
});

test("partial API equivalents state their coverage without an asterisk", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, usageUnpricedTokens: 7 });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();

  const metric = page.locator(".usage-metrics > div").filter({ hasText: "API equivalent" });
  await expect(metric).toContainText("21 priced · 7 unpriced");
  await expect(metric).not.toContainText("*");
  await expect(page.locator('.usage-request-table tbody td[data-column="equivalent"]')).not.toContainText("*");
});

test("OAuth member policy hides manual routing controls", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Pool member policy: Personal Plus", exact: true }).click();

  const dialog = page.getByRole("dialog", { name: /Pool member policy/ });
  await expect(dialog).not.toContainText("Tie-break priority");
  await expect(dialog).not.toContainText("Traffic share");
  await expect(dialog.getByRole("tab", { name: "Models", exact: true })).toHaveAttribute("aria-selected", "true");
  await expect(dialog.locator("[data-member-model-id]")).toHaveCount(2);
  await dialog.getByRole("tab", { name: "Settings", exact: true }).click();
  await expect(dialog.getByRole("switch", { name: "Drain", exact: true })).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Remove from pool" })).toHaveCount(0);
});

for (const mode of ["local", "remote"] as const) {
  test(`${mode} model rules keep catalog order and toggle the same runtime contract`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true, accountCount: 2 });
    await page.goto("/");
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await page.getByRole("tab", { name: "Model Rules" }).click();

    const rows = page.locator(".model-rules tbody tr[data-model-id]");
    await expect(rows).toHaveCount(3);
    // Rules keep the order delivered by the runtime catalog. Relay does not
    // apply the launcher's semantic model sorting here.
    expect(await rows.evaluateAll((items) => items.map((item) => item.getAttribute("data-model-id")))).toEqual(["gpt-5.4", "gpt-5.4-mini", "o3"]);
    const firstGroup = page.locator(".model-rules tbody").first();
    const groupModels = firstGroup.locator("tr[data-model-id]");
    const groupModelCount = await groupModels.count();
    await firstGroup.locator(".model-group-toggle").click();
    await expect(firstGroup.locator(".model-group-toggle")).toHaveAttribute("aria-expanded", "false");
    await expect(groupModels).toHaveCount(0);
    await firstGroup.locator(".model-group-toggle").click();
    await expect(firstGroup.locator(".model-group-toggle")).toHaveAttribute("aria-expanded", "true");
    await expect(groupModels).toHaveCount(groupModelCount);
    await expect(rows.first().locator("[data-column='availability']")).toHaveCount(0);
    await expect(page.locator('.model-rules [data-column="price"]')).toHaveCount(0);
    await expect(page.locator(".model-rules")).not.toContainText("Price not listed");

    await expect(page.locator(".model-sort-select")).toHaveCount(0);

    const mini = page.locator('.model-rules tbody tr[data-model-id="gpt-5.4-mini"]');
    await mini.getByRole("checkbox", { name: "Disable gpt-5.4-mini" }).click();
    await expect(mini).toHaveAttribute("data-enabled", "false");
    await expect(mini.getByRole("checkbox", { name: "Enable gpt-5.4-mini" })).not.toBeChecked();
    await mini.getByRole("checkbox", { name: "Enable gpt-5.4-mini" }).click();
    await expect(mini).toHaveAttribute("data-enabled", "true");

    const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
    if (mode === "local") {
      expect(calls.filter((call) => call.command === "set_local_model_enabled").map((call) => call.args)).toEqual([
        { input: { modelId: "gpt-5.4-mini", enabled: false } },
        { input: { modelId: "gpt-5.4-mini", enabled: true } },
      ]);
    } else {
      expect(calls.filter((call) => call.command === "execute_remote_server_action" && (call.args.input as { action?: { type?: string } } | undefined)?.action?.type === "set_model_enabled").map((call) => call.args)).toEqual([
        { input: { action: { type: "set_model_enabled" }, payload: { modelId: "gpt-5.4-mini", enabled: false } } },
        { input: { action: { type: "set_model_enabled" }, payload: { modelId: "gpt-5.4-mini", enabled: true } } },
      ]);
    }
  });
}

test("remote model rules preserve the server group and model order", async ({ page }) => {
  await installTauriMock(page, {
    mode: "remote",
    locale: "en",
    populated: true,
    accountModels: [],
    serverModelOrder: [
      "gemini-3.6-flash-high",
      "gemini-3.6-flash-medium",
      "gemini-3.6-flash-low",
    ],
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("tab", { name: "Model Rules" }).click();

  const rows = page.locator(".model-rules tbody tr[data-model-id]");
  expect(await rows.evaluateAll((items) => items.map((item) => item.getAttribute("data-model-id")))).toEqual([
    "gemini-3.6-flash-high",
    "gemini-3.6-flash-medium",
    "gemini-3.6-flash-low",
  ]);
});

for (const mode of ["local", "remote"] as const) {
  test(`${mode} model rules persist configured request speed and drag order`, async ({ page }) => {
    await installTauriMock(page, {
      mode,
      locale: "en",
      populated: true,
      mixedModels: true,
      modelSpeed: { "gpt-5.4": "standard" },
      modelReasoning: { "gpt-5.4": ["low", "medium", "high"] },
    });
    await page.goto("/");
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await page.getByRole("tab", { name: "Model Rules" }).click();

    const model = page.locator('.model-rules tbody tr[data-model-id="gpt-5.4"]');
    const speed = model.locator(".model-speed-toggle");
    await expect(speed).toBeVisible();
    await expect(model.locator(".model-rule-actions")).toHaveCSS("opacity", "1");
    expect(await model.locator(".model-rule-actions > *").evaluateAll((controls) => controls.map((control) => {
      if (control.matches(".model-protocol-button")) return "protocol";
      if (control.matches(".model-toggle")) return "enabled";
      if (control.matches(".model-speed-toggle")) return "speed";
      return control.matches("[data-model-reasoning-edit]") || control.querySelector("[data-model-reasoning-edit]") ? "reasoning" : "unknown";
    }))).toEqual(["speed", "reasoning", "enabled"]);
    await expect(speed).toHaveAttribute("data-speed-tier", "standard");
    await expect(speed.getByRole("radio", { name: "Standard", exact: true })).toBeChecked();
    await speed.getByRole("radio", { name: "Fast", exact: true }).click();
    await expect(speed).toHaveAttribute("data-speed-tier", "fast");
    await expect(speed.getByRole("radio", { name: "Fast", exact: true })).toBeChecked();
    const claude = page.locator('.model-rules tbody tr[data-model-id="claude-opus-4-8"]');
    await expect(claude.locator(".model-speed-toggle")).toHaveCount(0);

    const groups = page.locator(".model-rules .model-group-row");
    await groups.first().dragTo(groups.last());
    const rows = page.locator(".model-rules tbody tr[data-model-id]");
    await rows.first().dragTo(rows.last());
    const calls = await page.evaluate(() => (window as unknown as {
      __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }>;
    }).__TAURI_TEST_INVOKES__);
    if (mode === "local") {
      expect(calls.some((call) => call.command === "set_local_model_service_tier" && call.args.input && (call.args.input as { modelId: string; serviceTier: string }).serviceTier === "fast")).toBe(true);
      expect(calls.some((call) => call.command === "set_local_model_display_order")).toBe(true);
    } else {
      expect(calls.some((call) => call.command === "execute_remote_server_action" && (call.args.input as { action?: { type?: string }; payload?: { serviceTier?: string } }).action?.type === "set_model_service_tier" && (call.args.input as { payload?: { serviceTier?: string } }).payload?.serviceTier === "fast")).toBe(true);
      expect(calls.some((call) => call.command === "execute_remote_server_action" && (call.args.input as { action?: { type?: string } }).action?.type === "set_model_order")).toBe(true);
    }
  });
}

test("local model reasoning defaults are compact and use backend modes", async ({ page }) => {
  await installTauriMock(page, {
    mode: "local",
    locale: "en",
    populated: true,
    mixedModels: true,
    modelReasoning: { "claude-opus-4-8": ["low", "medium", "high", "xhigh", "max"] },
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("tab", { name: "Model Rules" }).click();

  const claude = page.locator('.model-rules tbody tr[data-model-id="claude-opus-4-8"]');
  await claude.getByRole("button", { name: "Set reasoning modes for claude-opus-4-8" }).click();
  const dialog = page.getByRole("dialog", { name: "Reasoning modes" });
  await expect(dialog.locator(".model-reasoning-model")).toHaveText("claude-opus-4-8");
  await expect(dialog.locator(".model-reasoning-company")).toHaveCount(0);
  await expect(dialog.getByLabel("Custom mode")).toHaveCount(0);
  await expect(dialog.getByRole("button", { name: "Add", exact: true })).toHaveCount(0);
  await page.screenshot({ path: "output/playwright/model-reasoning-dialog-compact-en-840x560.png" });
  const max = dialog.getByRole("checkbox", { name: "Max" });
  await expect(max).toHaveAttribute("aria-checked", "true");
  await expect(dialog.getByRole("button", { name: "Save" })).toHaveCount(0);

  await max.click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((call) => call.command === "set_local_model_reasoning")?.args)).toEqual({
    input: { modelId: "claude-opus-4-8", allowedLevels: ["low", "medium", "high", "xhigh"] },
  });
  await expect(max).toHaveAttribute("aria-checked", "false");
});

test("model availability errors stay visible", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, catalogRefreshWarning: "failed" });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("tab", { name: "Model Rules" }).click();
  const alert = page.locator(".model-discovery-alert");
  await expect(alert).toHaveCount(0);
});

test("provider discovery errors stay local when account models are available", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, sourceErrorCode: "upstream model discovery failed" });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("tab", { name: "Model Rules" }).click();
  await expect(page.locator(".model-discovery-alert")).toHaveCount(0);
});

test("deferred model availability checks stay quiet when account models are available", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, catalogRefreshWarning: "deferred" });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("tab", { name: "Model Rules" }).click();
  await expect(page.locator(".model-discovery-alert")).toHaveCount(0);
});

test("every native pool model exposes backend reasoning settings", async ({ page }) => {
  await installTauriMock(page, {
    mode: "local",
    locale: "en",
    populated: true,
    serverModelOrder: [],
    quotaAvailable: true,
    modelReasoning: { "gpt-5.4": ["low", "medium", "high"] },
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("tab", { name: "Model Rules" }).click();

  const model = page.locator('.model-rules tbody tr[data-model-id="gpt-5.4"]');
  await model.getByRole("button", { name: "Set reasoning modes for gpt-5.4" }).click();
  const dialog = page.getByRole("dialog", { name: "Reasoning modes" });
  await expect(dialog.locator(".model-reasoning-model")).toHaveText("gpt-5.4");
  await expect(dialog.getByRole("checkbox")).toHaveText(["Low", "Medium", "High"]);
  await expect(dialog.getByLabel("Custom mode")).toHaveCount(0);
  await dialog.getByRole("checkbox", { name: "High" }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((call) => call.command === "set_local_model_reasoning")?.args)).toEqual({
    input: { modelId: "gpt-5.4", allowedLevels: ["low", "medium"] },
  });
});

test("normalized catalog levels expose the complete Fable reasoning enum", async ({ page }) => {
  await installTauriMock(page, {
    mode: "local",
    locale: "en",
    populated: true,
    serverModelOrder: ["claude-fable-5-1"],
    modelReasoning: {
      "claude-fable-5-1": ["low", "medium", "high", "xhigh", "max"],
    },
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("tab", { name: "Model Rules" }).click();

  const model = page.locator('.model-rules tbody tr[data-model-id="claude-fable-5-1"]');
  await model.getByRole("button", { name: "Set reasoning modes for claude-fable-5-1" }).click();
  const dialog = page.getByRole("dialog", { name: "Reasoning modes" });
  await expect(dialog.getByRole("checkbox")).toHaveText(["Low", "Medium", "High", "Extra high", "Max"]);
  await dialog.getByRole("checkbox", { name: "High", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((call) => call.command === "set_local_model_reasoning")?.args)).toEqual({
    input: { modelId: "claude-fable-5-1", allowedLevels: ["low", "medium", "xhigh", "max"] },
  });
});

test("remote reasoning toggles are applied without a Save button", async ({ page }) => {
  await installTauriMock(page, {
    mode: "remote",
    locale: "en",
    populated: true,
    mixedModels: true,
    modelReasoning: { "claude-opus-4-8": ["low", "high", "ultra"] },
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("tab", { name: "Model Rules" }).click();

  const claude = page.locator('.model-rules tbody tr[data-model-id="claude-opus-4-8"]');
  await claude.getByRole("button", { name: "Set reasoning modes for claude-opus-4-8" }).click();
  const dialog = page.getByRole("dialog", { name: "Reasoning modes" });
  await dialog.getByRole("checkbox", { name: "Ultra" }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((call) => call.command === "execute_remote_server_action" && (call.args.input as { action?: { type?: string } } | undefined)?.action?.type === "set_model_reasoning")?.args)).toEqual({
    input: { action: { type: "set_model_reasoning" }, payload: { modelId: "claude-opus-4-8", allowedLevels: ["low", "high"] } },
  });
  await expect(dialog.getByRole("button", { name: "Save" })).toHaveCount(0);
});
