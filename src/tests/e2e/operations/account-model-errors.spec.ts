import { expect, test } from "../../bun-playwright";
import { installTauriMock } from "../tauri-mock";
import { connectPoolToChatGPT } from "./helpers";

test("empty pool catalogs expose a specific localized IPC error", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.evaluate(() => {
    const internals = (window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
    }).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    internals.invoke = (command, args, options) => command === "attach_codex_to_local_gateway"
      ? Promise.reject({
        code: "pool_models_unavailable",
        message: "Synthetic empty pool model catalog",
        diagnostic: { reason: "empty_model_catalog", route: "/v1/models" },
      })
      : invoke(command, args, options);
  });
  await connectPoolToChatGPT(page);
  const feedback = page.locator(".global-feedback.error");
  await feedback.locator(".global-feedback-error-trigger").click();
  const details = page.getByRole("dialog", { name: "Error details" });
  await expect(details).toContainText("The pool has no models available for ChatGPT");
  await expect(details).toContainText('"code": "pool_models_unavailable"');
  await expect(details).toContainText('"reason": "empty_model_catalog"');
  await expect(details).not.toContainText("The current state conflicts");
});

test("saved BPS model errors distinguish current proxy settings from unknown request context", async ({ page }) => {
  await installTauriMock(page, {
    mode: "local", locale: "en", populated: true, accountCount: 1,
    accountOAuthClientKind: "excel_bps", accountHealth: "degraded",
    accountModelErrorCode: "models_forbidden",
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const account = page.locator('[data-member-label="Personal Plus"]');
  await account.getByRole("button", { name: /HTTP 403/ }).click();
  const details = page.getByRole("dialog", { name: "Technical error details" });
  const payload = JSON.parse(await details.locator("pre").innerText());
  expect(payload).toMatchObject({
    code: "models_forbidden",
    connection_kind: "excel_bps",
    observed_at: null,
    failed_request_route: null,
    error_scope: "models",
    current_route: { proxy_mode: "common", configuration_valid: true },
  });
  expect(payload).not.toHaveProperty("proxy_mode");
  expect(payload).not.toHaveProperty("proxy_available");
});
