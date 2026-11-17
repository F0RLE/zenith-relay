import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

for (const locale of ["en", "ru"] as const) {
  for (const width of [1160, 390]) {
    for (const inPool of [false, true]) {
      test(`backend Free image restriction hides admission but preserves removal: ${locale} ${width} ${inPool}`, async ({ page }, testInfo) => {
        await page.setViewportSize({ width, height: 900 });
        await installTauriMock(page, { locale, populated: true, accountCount: 1, accountOAuthClientKind: "codex" });
        await page.addInitScript(({ inPool }) => {
          const native = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
          const invoke = native.invoke.bind(native);
          native.invoke = async (command, args, options) => {
            const result = await invoke(command, args, options);
            if (command === "get_local_runtime_state") {
              const snapshot = result as { accounts: import("../../src/features/relay/api/types").AccountSummary[] };
              for (const account of snapshot.accounts) {
                account.subscription.planType = "free";
                account.label = "Codex Free";
                account.inPool = inPool;
                account.models = [];
                account.poolEligible = false;
                account.imageGenerationRestriction = "provider_free_plan";
              }
            }
            return result;
          };
        }, { inPool });
        await page.goto("/");
        await page.getByRole("button", { name: locale === "ru" ? "Подключения" : "Connections", exact: true }).click();
        const card = page.locator(".account-card").first();
        await expect(card.getByText(/OpenAI provider restriction|ограничение провайдера OpenAI/)).toHaveCount(0);
        await expect(card.getByRole("button", { name: locale === "ru" ? "Добавить в пул для изображений" : "Add to pool for images", exact: true })).toHaveCount(0);
        const remove = card.getByRole("button", { name: locale === "ru" ? "Убрать из пула" : "Remove from pool", exact: true });
        if (inPool) await expect(remove).toBeEnabled();
        else await expect(remove).toHaveCount(0);
        expect(await card.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
        await page.screenshot({ path: testInfo.outputPath("free-codex.png"), animations: "disabled" });
      });
    }
  }
}
