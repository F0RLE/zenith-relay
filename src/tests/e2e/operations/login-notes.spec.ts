import { expect, test, type Locator, type Page } from "../../bun-playwright";
import type { AccountLoginDraft } from "../../../src/features/relay/accountLoginDraft";
import { installTauriMock } from "../tauri-mock";

type NotesMockState = {
  updates: AccountLoginDraft[];
  failSave: boolean;
  resolveLoad: () => void;
  pendingPreviews: Array<{ secret: string; resolve: (preview: { code: string | null; expiresAtMs: number | null }) => void }>;
};

type NotesTestWindow = Window & { __LOGIN_NOTES_TEST__: NotesMockState };

async function installNotesMock(page: Page, {
  values = {},
  theme = "dark",
  loading = "ready",
  deferredPreview = false,
  reauthentication = false,
}: {
  values?: Partial<AccountLoginDraft>;
  theme?: "dark" | "light";
  loading?: "ready" | "pending" | "failed";
  deferredPreview?: boolean;
  reauthentication?: boolean;
} = {}) {
  await installTauriMock(page, {
    mode: "local", locale: "ru", theme, populated: true,
    ...(reauthentication ? { accountAuthReason: "invalid_grant", quotaRefreshStatus: "requires_reauth" } : {}),
  });
  await page.addInitScript(({ values, loading, deferredPreview }) => {
    const testWindow = window as unknown as NotesTestWindow & {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: Record<string, unknown>, options?: unknown) => Promise<unknown> };
    };
    const stored = { email: "person@example.test", phone: "", password: "", totpSecret: "", ...values };
    const details = () => ({ accountId: "account_synthetic", ...stored, totpCode: null, totpExpiresAtMs: null });
    let resolveLoad = () => {};
    const load = loading === "pending" ? new Promise<void>((resolve) => { resolveLoad = resolve; }) : Promise.resolve();
    const state: NotesMockState = { updates: [], failSave: false, resolveLoad, pendingPreviews: [] };
    testWindow.__LOGIN_NOTES_TEST__ = state;
    const invoke = testWindow.__TAURI_INTERNALS__.invoke.bind(testWindow.__TAURI_INTERNALS__);
    testWindow.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
      if (command === "reveal_local_account_login") {
        if (loading === "failed") throw new Error("Synthetic notes load failure");
        await load;
        return details();
      }
      if (command === "update_local_account_login") {
        if (state.failSave) throw new Error("Synthetic notes save failure");
        const { email, phone, password, totpSecret } = args!.input as AccountLoginDraft;
        const values = { email, phone, password, totpSecret };
        state.updates.push(values);
        Object.assign(stored, values);
        return details();
      }
      if (command === "preview_totp_code") {
        if (deferredPreview) {
          return new Promise((resolve) => state.pendingPreviews.push({ secret: String(args!.secret), resolve }));
        }
        return { code: "123456", expiresAtMs: Date.now() + 30_000 };
      }
      return invoke(command, args, options);
    };
  }, { values, loading, deferredPreview });
}

async function openNotes(page: Page) {
  await page.getByRole("button", { name: "Подключения", exact: true }).click();
  await page.locator(".account-card .account-row-menu summary").first().click();
  await page.getByRole("menuitem", { name: "Заметки", exact: true }).click();
  return page.getByRole("dialog", { name: "Заметки", exact: true });
}

async function expectSecretGeometry(container: Locator) {
  expect(await container.locator(".secret-field").evaluateAll((fields) => fields.every((field) => {
    const input = field.querySelector("input")!;
    const button = field.querySelector("button")!;
    const inputBox = input.getBoundingClientRect();
    const buttonBox = button.getBoundingClientRect();
    const style = getComputedStyle(input);
    return buttonBox.width >= 32 && buttonBox.height >= 32
      && buttonBox.left >= inputBox.left && buttonBox.right <= inputBox.right
      && buttonBox.top >= inputBox.top && buttonBox.bottom <= inputBox.bottom
      && Number.parseFloat(style.paddingRight) >= buttonBox.width + 10
      && style.borderTopLeftRadius === style.borderTopRightRadius;
  }))).toBe(true);
}

for (const theme of ["dark", "light"] as const) {
  for (const width of [1160, 390]) {
    test(`notes use masked fields without clipping in ${theme} at ${width}px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 760 });
      await installNotesMock(page, { theme });
      await page.goto("/");
      const dialog = await openNotes(page);
      const password = dialog.getByLabel("Пароль", { exact: true });
      const secret = dialog.getByLabel("Секрет 2FA", { exact: true });
      await expect(password).toBeEnabled();
      await expect(password).toHaveAttribute("type", "password");
      await expect(secret).toHaveAttribute("type", "password");
      await expect(dialog.locator(".account-login-code")).toHaveCount(0);
      await expect(dialog.getByRole("button", { name: "Показать: Пароль", exact: true })).toBeVisible();
      await expect(dialog.getByRole("button", { name: "Показать: Секрет 2FA", exact: true })).toBeVisible();
      await expectSecretGeometry(dialog);
      expect(await dialog.evaluate((element) => {
        const body = element.querySelector(".relay-dialog-body")!;
        const fields = element.querySelector(".account-login-fields")!;
        const box = element.getBoundingClientRect();
        return body.scrollWidth <= body.clientWidth && fields.scrollWidth <= fields.clientWidth
          && box.left >= 0 && box.right <= innerWidth && box.top >= 36 && box.bottom <= innerHeight;
      })).toBe(true);
      await page.screenshot({ path: `output/playwright/login-notes-${theme}-${width}.png` });
    });
  }
}

test("revealing and saving notes preserves other fields and resets visibility on reopen", async ({ page }) => {
  await installNotesMock(page, { values: { phone: "+15550000123" } });
  await page.goto("/");
  let dialog = await openNotes(page);
  const password = dialog.getByLabel("Пароль", { exact: true });
  await password.fill("synthetic-password");
  const beforeSave = await dialog.boundingBox();
  await dialog.getByRole("button", { name: "Показать: Пароль", exact: true }).click();
  await expect(password).toHaveAttribute("type", "text");
  await expect(dialog.getByRole("button", { name: "Скрыть: Пароль", exact: true })).toHaveAttribute("aria-pressed", "true");
  await expect.poll(() => page.evaluate(() => (window as NotesTestWindow).__LOGIN_NOTES_TEST__.updates)).toEqual([{
    email: "person@example.test", phone: "+15550000123", password: "synthetic-password", totpSecret: "",
  }]);
  expect(await dialog.boundingBox()).toEqual(beforeSave);
  await dialog.getByRole("button", { name: "Скрыть: Пароль", exact: true }).click();
  await expect(password).toHaveAttribute("type", "password");

  await dialog.getByLabel("Секрет 2FA", { exact: true }).fill("GEZDGNBVGY3TQOJQ");
  await expect(dialog.locator(".account-login-code strong")).toHaveText("123456");
  await expectSecretGeometry(dialog);
  await dialog.locator(".account-login-copy").click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe("123456");
  await expect(dialog.locator(".account-login-copy")).toContainText("Скопировано");
  await dialog.getByLabel("Пароль", { exact: true }).fill("synthetic-new-password");
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => (window as NotesTestWindow).__LOGIN_NOTES_TEST__.updates.at(-1))).toEqual({
    email: "person@example.test", phone: "+15550000123", password: "synthetic-new-password", totpSecret: "GEZDGNBVGY3TQOJQ",
  });

  dialog = await openNotes(page);
  await expect(dialog.getByLabel("Пароль", { exact: true })).toHaveValue("synthetic-new-password");
  await expect(dialog.getByLabel("Пароль", { exact: true })).toHaveAttribute("type", "password");
  await expect(dialog.getByLabel("Секрет 2FA", { exact: true })).toHaveAttribute("type", "password");
  await dialog.getByLabel("Секрет 2FA", { exact: true }).fill("");
  await expect(dialog.locator(".account-login-code")).toHaveCount(0);
  await dialog.getByLabel("Почта", { exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as NotesTestWindow).__LOGIN_NOTES_TEST__.updates.at(-1)?.totpSecret)).toBe("");
});

test("notes disable both secret controls until loading finishes", async ({ page }) => {
  await installNotesMock(page, { loading: "pending", values: { password: "synthetic-password" } });
  await page.goto("/");
  const dialog = await openNotes(page);
  await expect(dialog.getByLabel("Пароль", { exact: true })).toBeDisabled();
  await expect(dialog.getByRole("button", { name: "Показать: Пароль", exact: true })).toBeDisabled();
  await expect(dialog.getByRole("button", { name: "Показать: Секрет 2FA", exact: true })).toBeDisabled();
  await expectSecretGeometry(dialog);
  await page.evaluate(() => (window as NotesTestWindow).__LOGIN_NOTES_TEST__.resolveLoad());
  await expect(dialog.getByLabel("Пароль", { exact: true })).toHaveValue("synthetic-password");
  await expect(dialog.getByRole("button", { name: "Показать: Пароль", exact: true })).toBeEnabled();
});

test("notes distinguish loading failures from saving failures", async ({ page }) => {
  await installNotesMock(page, { loading: "failed" });
  await page.goto("/");
  let dialog = await openNotes(page);
  await expect(dialog.getByRole("alert")).toHaveText("Не удалось загрузить заметки");
  await expect(dialog.getByLabel("Пароль", { exact: true })).toBeDisabled();
  await expect(dialog.getByRole("button", { name: "Показать: Пароль", exact: true })).toBeDisabled();
  await expect(dialog.getByRole("status")).toHaveCount(0);
  await page.keyboard.press("Escape");

  // A fresh navigation installs a functioning load path while keeping this
  // independent from the component's load-failure state.
  await installNotesMock(page);
  await page.goto("/");
  dialog = await openNotes(page);
  await page.evaluate(() => { (window as NotesTestWindow).__LOGIN_NOTES_TEST__.failSave = true; });
  await dialog.getByLabel("Пароль", { exact: true }).fill("synthetic-password");
  await dialog.getByLabel("Почта", { exact: true }).click();
  await expect(dialog.getByRole("alert")).toHaveText("Не удалось сохранить заметки");
  await expect(dialog.getByRole("status")).toHaveCount(0);
  await expect(dialog.getByLabel("Пароль", { exact: true })).toHaveValue("synthetic-password");
  await page.evaluate(() => { (window as NotesTestWindow).__LOGIN_NOTES_TEST__.failSave = false; });
  await dialog.getByLabel("Пароль", { exact: true }).click();
  await dialog.getByLabel("Почта", { exact: true }).click();
  await expect(dialog.getByRole("status")).toHaveText("Сохранено");
  await expect(dialog.getByRole("alert")).toHaveCount(0);
});

test("changing a 2FA secret clears the old code while the new preview is pending", async ({ page }) => {
  await installNotesMock(page, { deferredPreview: true, values: { totpSecret: "GEZDGNBVGY3TQOJQ" } });
  await page.goto("/");
  const dialog = await openNotes(page);
  await expect.poll(() => page.evaluate(() => (window as NotesTestWindow).__LOGIN_NOTES_TEST__.pendingPreviews.length)).toBe(1);
  await page.evaluate(() => (window as NotesTestWindow).__LOGIN_NOTES_TEST__.pendingPreviews.shift()!.resolve({
    code: "123456", expiresAtMs: Date.now() + 30_000,
  }));
  await expect(dialog.locator(".account-login-code strong")).toHaveText("123456");
  await dialog.getByLabel("Секрет 2FA", { exact: true }).fill("JBSWY3DPEHPK3PXP");
  await expect(dialog.locator(".account-login-code strong")).toHaveText("—");
  await expect(dialog.locator(".account-login-copy")).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => (window as NotesTestWindow).__LOGIN_NOTES_TEST__.pendingPreviews.length)).toBe(1);
  await dialog.getByLabel("Секрет 2FA", { exact: true }).fill("");
  await page.evaluate(() => (window as NotesTestWindow).__LOGIN_NOTES_TEST__.pendingPreviews.shift()!.resolve({
    code: "654321", expiresAtMs: Date.now() + 30_000,
  }));
  await expect(dialog.locator(".account-login-code")).toHaveCount(0);
});

test("reauthentication notes reuse masked fields and leave unedited notes intact", async ({ page }) => {
  await installNotesMock(page, { reauthentication: true, values: { password: "synthetic-password", totpSecret: "GEZDGNBVGY3TQOJQ" } });
  await page.goto("/");
  await page.getByRole("button", { name: "Подключения", exact: true }).click();
  await page.locator(".account-card").first().getByRole("button", { name: "Требуется вход", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Войти", exact: true });
  await dialog.getByRole("button", { name: "Заметки", exact: true }).click();
  await expect(dialog.getByLabel("Пароль", { exact: true })).toHaveValue("synthetic-password");
  await expect(dialog.getByLabel("Пароль", { exact: true })).toHaveAttribute("type", "password");
  await expect(dialog.getByLabel("Секрет 2FA", { exact: true })).toHaveAttribute("type", "password");
  await expectSecretGeometry(dialog);
  await dialog.getByRole("button", { name: "Показать: Пароль", exact: true }).click();
  await dialog.getByRole("button", { name: "Заметки", exact: true }).click();
  expect(await page.evaluate(() => (window as NotesTestWindow).__LOGIN_NOTES_TEST__.updates)).toEqual([]);
  await dialog.getByRole("button", { name: "Заметки", exact: true }).click();
  await expect(dialog.getByLabel("Пароль", { exact: true })).toHaveAttribute("type", "password");
});
