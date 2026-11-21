import type { Page } from "../bun-playwright";
import type { MockOptions } from "./tauri-mock/options";
import { initializeTauriMock } from "./tauri-mock/runtime";

export type { MockOptions } from "./tauri-mock/options";

export async function emitTauriEvent(page: Page, event: string, payload: unknown) {
  await page.evaluate(([name, value]) => {
    (window as unknown as { __TAURI_TEST_EMIT__: (event: string, payload: unknown) => void }).__TAURI_TEST_EMIT__(name, value);
  }, [event, payload] as const);
}

export async function installTauriMock(page: Page, options: MockOptions = {}) {
  await page.addInitScript(initializeTauriMock, options);
}
