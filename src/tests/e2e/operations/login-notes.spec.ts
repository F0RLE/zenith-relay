import { expect, test, type Locator, type Page } from "../../bun-playwright";
import type { AccountLoginDraft } from "../../../src/features/relay/accountLoginDraft";
import type { OAuthClientKind } from "../../../src/features/relay/api/types";
import { emitTauriEvent, installTauriMock } from "../tauri-mock";

type NotesMockState = {
  browserClipboardWrites: number;
  updates: AccountLoginDraft[];
  updatedAccounts: string[];
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
  populated = true,
  accountRouteKinds,
  oauthCompletedRouteKind,
}: {
  values?: Partial<AccountLoginDraft>;
  theme?: "dark" | "light" | "system";
  loading?: "ready" | "pending" | "failed";
  deferredPreview?: boolean;
  reauthentication?: boolean;
  populated?: boolean;
  accountRouteKinds?: OAuthClientKind[];
  oauthCompletedRouteKind?: OAuthClientKind;
} = {}) {
  await installTauriMock(page, {
    mode: "local", locale: "ru", theme, populated,
    ...(accountRouteKinds ? { accountRouteKinds, usageActive: false } : {}),
    ...(oauthCompletedRouteKind ? { oauthCompletedRouteKind } : {}),
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
    const state: NotesMockState = { browserClipboardWrites: 0, updates: [], updatedAccounts: [], failSave: false, resolveLoad, pendingPreviews: [] };
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
        state.updatedAccounts.push(String((args!.input as { accountId: string }).accountId));
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

async function blockClipboardPermissionApi(page: Page) {
  await page.evaluate(() => {
    Object.defineProperty(navigator.clipboard, "writeText", { configurable: true, value: async () => {
      (window as NotesTestWindow).__LOGIN_NOTES_TEST__.browserClipboardWrites += 1;
      throw new Error("Synthetic Clipboard API permission denied");
    } });
  });
}

test("masked notes copy full and partial selections and copy buttons without clipboard permission prompts", async ({ page }) => {
  const passwordValue = "synthetic-password";
  const totpValue = "JBSWY3DPEHPK3PXP";
  await installNotesMock(page, { values: { password: passwordValue, totpSecret: totpValue } });
  await page.goto("/");
  const dialog = await openNotes(page);
  await blockClipboardPermissionApi(page);
  for (const [label, value] of [["Пароль", passwordValue], ["Секрет 2FA", totpValue]] as const) {
    const field = dialog.getByLabel(label, { exact: true });
    await expect(field).toHaveAttribute("type", "password");
    await field.focus();
    await field.press("ControlOrMeta+A");
    await field.press("ControlOrMeta+C");
    await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(value);
    await expect(field).toBeFocused();
    expect(await field.evaluate((input: HTMLInputElement) => [input.selectionStart, input.selectionEnd])).toEqual([0, value.length]);

    await field.evaluate((input: HTMLInputElement) => input.setSelectionRange(2, 8));
    await field.press("ControlOrMeta+C");
    await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(value.slice(2, 8));
    expect(await field.evaluate((input: HTMLInputElement) => [input.selectionStart, input.selectionEnd])).toEqual([2, 8]);
    await field.evaluate((input: HTMLInputElement) => input.setSelectionRange(3, 3));
    await field.press("ControlOrMeta+C");
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(value.slice(2, 8));

    await dialog.getByRole("button", { name: new RegExp(`^Копировать: ${label}`) }).click();
    await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(value);
    await expect(field).toHaveAttribute("type", "password");
    await expect(field).toHaveValue(value);
    await expect(dialog.getByRole("button", { name: `Показать: ${label}`, exact: true })).toHaveAttribute("aria-pressed", "false");
  }
  await expectSecretGeometry(dialog);
  expect(await page.evaluate(() => (window as NotesTestWindow).__LOGIN_NOTES_TEST__.browserClipboardWrites)).toBe(0);
  await expect(page.locator("body > textarea[aria-hidden='true']")).toHaveCount(0);
  await expect(page.locator(".app-context-menu")).toHaveCount(0);
});

test("a selected masked secret also copies through the app context menu", async ({ page }) => {
  await installNotesMock(page, { values: { password: "synthetic-context-password" } });
  await page.goto("/");
  const dialog = await openNotes(page);
  await blockClipboardPermissionApi(page);
  const password = dialog.getByLabel("Пароль", { exact: true });
  await password.focus();
  await password.press("ControlOrMeta+A");
  await password.click({ button: "right" });
  const copy = page.getByRole("menu").getByRole("menuitem", { name: "Копировать", exact: true });
  await expect(copy).toBeEnabled();
  await copy.click();
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe("synthetic-context-password");
  await expect(password).toHaveAttribute("type", "password");
  await expect(password).toBeFocused();
  expect(await page.evaluate(() => (window as NotesTestWindow).__LOGIN_NOTES_TEST__.browserClipboardWrites)).toBe(0);
});

test("copy failure never opens the Clipboard API or reports copied and removes the temporary field", async ({ page }) => {
  await installNotesMock(page, { values: { password: "synthetic-password" } });
  await page.goto("/");
  const dialog = await openNotes(page);
  await blockClipboardPermissionApi(page);
  await page.evaluate(() => { document.execCommand = () => false; });
  await dialog.getByRole("button", { name: "Копировать: Пароль", exact: true }).click();
  await expect(dialog.getByRole("button", { name: "Копировать: Пароль: Не удалось скопировать", exact: true })).toHaveAttribute("data-copied", "false");
  await expect(dialog.getByLabel("Пароль", { exact: true })).toHaveAttribute("type", "password");
  await expect(page.locator("body > textarea[aria-hidden='true']")).toHaveCount(0);
  expect(await page.evaluate(() => (window as NotesTestWindow).__LOGIN_NOTES_TEST__.browserClipboardWrites)).toBe(0);
});

async function expectSecretGeometry(container: Locator) {
  expect(await container.locator(".secret-field").evaluateAll((fields) => fields.every((field) => {
    const input = field.querySelector("input")!;
    const buttons = Array.from(field.querySelectorAll("button"));
    const inputBox = input.getBoundingClientRect();
    const style = getComputedStyle(input);
    const boxes = buttons.map((button) => button.getBoundingClientRect());
    return boxes.length === 2 && boxes.every((buttonBox) => buttonBox.width >= 32 && buttonBox.height >= 32
      && buttonBox.left >= inputBox.left && buttonBox.right <= inputBox.right
      && buttonBox.top >= inputBox.top && buttonBox.bottom <= inputBox.bottom)
      && boxes[0].right <= boxes[1].left
      && Number.parseFloat(style.paddingRight) >= boxes.reduce((sum, box) => sum + box.width, 0) + 10
      && style.borderTopLeftRadius === style.borderTopRightRadius;
  }))).toBe(true);
}

async function expectNeutralSelection(field: Locator, dark: boolean) {
  await field.focus();
  await field.press("ControlOrMeta+A");
  await expect.poll(() => field.evaluate((element) => getComputedStyle(element).backgroundColor)).toBe(dark ? "rgb(32, 32, 32)" : "rgb(238, 238, 238)");
  const selected = await field.evaluate((element: HTMLInputElement | HTMLTextAreaElement) => {
    const style = getComputedStyle(element, "::selection");
    const luminance = (color: string) => color.match(/\d+/g)!.slice(0, 3).map(Number).map((channel) => {
      const value = channel / 255;
      return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
    }).reduce((sum, value, index) => sum + value * [0.2126, 0.7152, 0.0722][index], 0);
    const contrast = (left: string, right: string) => {
      const a = luminance(left), b = luminance(right);
      return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
    };
    return {
      background: style.backgroundColor,
      color: style.color,
      selected: element.selectionStart === 0 && element.selectionEnd === element.value.length && element.value.length > 0,
      textContrast: contrast(style.color, style.backgroundColor),
      selectionContrast: contrast(style.backgroundColor, getComputedStyle(element).backgroundColor),
    };
  });
  expect(selected.background).toBe(dark ? "rgb(56, 56, 56)" : "rgb(216, 216, 216)");
  expect(selected.color).toBe(dark ? "rgb(242, 240, 235)" : "rgb(32, 43, 41)");
  expect(selected.selected).toBe(true);
  expect(selected.textContrast).toBeGreaterThanOrEqual(4.5);
  expect(selected.selectionContrast).toBeGreaterThanOrEqual(1.1);
  expect(selected.selectionContrast).toBeLessThanOrEqual(1.7);
}

for (const appearance of [
  { theme: "dark", colorScheme: "light", dark: true },
  { theme: "light", colorScheme: "dark", dark: false },
  { theme: "system", colorScheme: "dark", dark: true },
  { theme: "system", colorScheme: "light", dark: false },
] as const) {
  test(`text selection and focused fields are neutral gray in ${appearance.theme}/${appearance.colorScheme}`, async ({ page }) => {
    await page.emulateMedia({ colorScheme: appearance.colorScheme });
    await installNotesMock(page, { theme: appearance.theme, values: { phone: "+15550000123", password: "synthetic-password", totpSecret: "JBSWY3DPEHPK3PXP" } });
    await page.goto("/");
    const dialog = await openNotes(page);
    await expect(dialog.getByLabel("Почта", { exact: true })).toBeEnabled();
    for (const field of await dialog.locator("input").all()) await expectNeutralSelection(field, appearance.dark);
    const email = dialog.getByLabel("Почта", { exact: true });
    await expectNeutralSelection(email, appearance.dark);
    await page.mouse.move(0, 0);
    await dialog.screenshot({ path: `output/playwright/neutral-selection-${appearance.theme}-${appearance.colorScheme}.png` });
    await dialog.getByRole("button", { name: "Закрыть", exact: true }).click();
    await page.getByRole("button", { name: "Импорт", exact: true }).click();
    const textarea = page.locator(".import-start textarea");
    await textarea.fill("synthetic import selection");
    await expectNeutralSelection(textarea, appearance.dark);
  });
}

for (const theme of ["dark", "light"] as const) {
  for (const width of [1160, 840, 390]) {
    test(`notes use masked fields without clipping in ${theme} at ${width}px`, async ({ page }) => {
      await page.setViewportSize({ width, height: width === 840 ? 560 : 760 });
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
      await expect(dialog.locator("footer")).toHaveCount(0);
      await expect(dialog.getByRole("button", { name: "Закрыть", exact: true })).toHaveCount(1);
      await expect(dialog.locator(".account-login-feedback")).toHaveCount(0);
      await expectSecretGeometry(dialog);
      expect(await dialog.evaluate((element) => {
        const body = element.querySelector(".relay-dialog-body")!;
        const fields = element.querySelector(".account-login-fields")!;
        const box = element.getBoundingClientRect();
        return body.scrollWidth <= body.clientWidth && fields.scrollWidth <= fields.clientWidth
          && box.left >= 0 && box.right <= innerWidth && box.top >= 36 && box.bottom <= innerHeight;
      })).toBe(true);
      const spacing = await dialog.evaluate((element) => {
        const body = element.querySelector(".relay-dialog-body")!;
        const fields = element.querySelector(".account-login-fields")!.getBoundingClientRect();
        return {
          bottomGap: body.getBoundingClientRect().bottom - fields.bottom,
          bodyScrolls: body.scrollHeight > body.clientHeight + 1,
        };
      });
      expect(spacing.bottomGap).toBeGreaterThanOrEqual(8);
      expect(spacing.bottomGap).toBeLessThanOrEqual(24);
      expect(spacing.bodyScrolls).toBe(false);
      await page.mouse.move(0, 0);
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
  await expect(dialog.getByText("Сохранено", { exact: true })).toHaveCount(0);
  await expect(dialog.getByRole("status")).toHaveCount(0);
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
  await expect(dialog.getByRole("button", { name: "Копировать: Пароль", exact: true })).toBeDisabled();
  await expect(dialog.getByRole("button", { name: "Копировать: Секрет 2FA", exact: true })).toBeDisabled();
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
  await expect.poll(() => page.evaluate(() => (window as NotesTestWindow).__LOGIN_NOTES_TEST__.updates.at(-1)?.password)).toBe("synthetic-password");
  await expect(dialog.getByRole("status")).toHaveCount(0);
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

for (const client of ["Codex", "ChatGPT (SIWC)", "Excel / Basis Points"]) {
  test(`new ${client} sign-in keeps pre-login notes and attaches them only after completion`, async ({ page }, testInfo) => {
    if (client === "Excel / Basis Points") await page.setViewportSize({ width: 390, height: 560 });
    await installNotesMock(page, { populated: false });
    await page.goto("/");
    await page.getByRole("button", { name: "Подключения", exact: true }).click();
    await page.getByRole("button", { name: "Войти", exact: true }).first().click();
    const dialog = page.getByRole("dialog", { name: "Войти", exact: true });
    await dialog.getByText(client, { exact: true }).click();
    await dialog.getByRole("button", { name: "Заметки", exact: true }).click();
    const values = { email: "manual@example.test", phone: "+15550000123", password: "synthetic-password", totpSecret: "GEZDGNBVGY3TQOJQ" };
    await dialog.getByLabel("Почта", { exact: true }).fill(values.email);
    await dialog.getByLabel("Телефон", { exact: true }).fill(values.phone);
    await dialog.getByLabel("Пароль", { exact: true }).fill(values.password);
    await dialog.getByLabel("Секрет 2FA", { exact: true }).fill(values.totpSecret);
    await expect(dialog.locator(".account-login-code strong")).toHaveText("123456");
    await expectSecretGeometry(dialog);
    expect(await page.evaluate(() => (window as NotesTestWindow).__LOGIN_NOTES_TEST__.updates)).toEqual([]);
    await dialog.getByRole("button", { name: "Заметки", exact: true }).click();
    await dialog.getByRole("button", { name: "Заметки", exact: true }).click();
    await expect(dialog.getByLabel("Почта", { exact: true })).toHaveValue(values.email);
    await expect(dialog.getByLabel("Телефон", { exact: true })).toHaveValue(values.phone);
    await expect(dialog.getByLabel("Пароль", { exact: true })).toHaveValue(values.password);
    await expect(dialog.getByLabel("Пароль", { exact: true })).toHaveAttribute("type", "password");
    await expect(dialog.getByLabel("Секрет 2FA", { exact: true })).toHaveValue(values.totpSecret);
    expect(await dialog.evaluate((element) => {
      const box = element.getBoundingClientRect();
      const body = element.querySelector(".relay-dialog-body")!;
      return box.left >= 0 && box.right <= innerWidth && box.top >= 36 && box.bottom <= innerHeight
        && body.scrollWidth <= body.clientWidth;
    })).toBe(true);
    await page.screenshot({ path: testInfo.outputPath("pre-login-notes.png") });
    await emitTauriEvent(page, "relay-oauth-status", { loginId: "oauth_synthetic", status: "callback_received" });
    await expect(dialog).toHaveCount(0);
    await expect.poll(() => page.evaluate(() => (window as NotesTestWindow).__LOGIN_NOTES_TEST__.updates)).toEqual([values]);
    expect(await page.evaluate(() => (window as NotesTestWindow).__LOGIN_NOTES_TEST__.updatedAccounts)).toEqual(["account_synthetic"]);
    const saved = await openNotes(page);
    await expect(saved.getByLabel("Почта", { exact: true })).toHaveValue(values.email);
    await expect(saved.getByLabel("Пароль", { exact: true })).toHaveValue(values.password);
    await expect(saved.getByLabel("Секрет 2FA", { exact: true })).toHaveValue(values.totpSecret);
  });
}

test("cancelling pre-login notes does not change an account or leak into the next sign-in", async ({ page }) => {
  await installNotesMock(page, { values: { password: "synthetic-stored-password" } });
  await page.goto("/");
  await page.getByRole("button", { name: "Подключения", exact: true }).click();
  await page.getByRole("button", { name: "Войти", exact: true }).first().click();
  let dialog = page.getByRole("dialog", { name: "Войти", exact: true });
  await dialog.getByRole("button", { name: "Заметки", exact: true }).click();
  await expect(dialog.getByLabel("Пароль", { exact: true })).toHaveValue("");
  await dialog.getByLabel("Пароль", { exact: true }).fill("synthetic-cancelled-password");
  await dialog.locator("header > button").click();
  await expect(dialog).toHaveCount(0);
  await page.getByRole("button", { name: "Войти", exact: true }).first().click();
  dialog = page.getByRole("dialog", { name: "Войти", exact: true });
  await dialog.getByRole("button", { name: "Заметки", exact: true }).click();
  await expect(dialog.getByLabel("Пароль", { exact: true })).toHaveValue("");
  await emitTauriEvent(page, "relay-oauth-status", { loginId: "oauth_synthetic", status: "callback_received" });
  await expect(dialog).toHaveCount(0);
  expect(await page.evaluate(() => (window as NotesTestWindow).__LOGIN_NOTES_TEST__.updates)).toEqual([]);
  const saved = await openNotes(page);
  await expect(saved.getByLabel("Пароль", { exact: true })).toHaveValue("synthetic-stored-password");
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

for (const [source, target, label] of [
  ["codex", "siwc", "SIWC"],
  ["siwc", "excel_bps", "Excel / BPS"],
  ["excel_bps", "codex", "Codex"],
] as const) {
  test(`adding ${target} to ${source} shows and edits the original card notes without reusing its grant`, async ({ page }) => {
    const values = { email: "person@example.test", phone: "+15550000123", password: "synthetic-source-password", totpSecret: "GEZDGNBVGY3TQOJQ" };
    await installNotesMock(page, { values, accountRouteKinds: [source], oauthCompletedRouteKind: target });
    await page.goto("/");
    await page.getByRole("button", { name: "Подключения", exact: true }).click();
    await page.locator(".account-route-selector > button").click();
    await page.getByRole("option", { name: `${label} · Подключить`, exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "Войти", exact: true });
    await dialog.getByRole("button", { name: "Заметки", exact: true }).click();
    await expect(dialog.getByLabel("Почта", { exact: true })).toHaveValue(values.email);
    await expect(dialog.getByLabel("Телефон", { exact: true })).toHaveValue(values.phone);
    await expect(dialog.getByLabel("Пароль", { exact: true })).toHaveValue(values.password);
    await expect(dialog.getByLabel("Секрет 2FA", { exact: true })).toHaveValue(values.totpSecret);
    await expect(dialog.getByLabel("Пароль", { exact: true })).toHaveAttribute("type", "password");
    await expect(dialog.getByLabel("Секрет 2FA", { exact: true })).toHaveAttribute("type", "password");
    expect(await page.evaluate(() => (window as NotesTestWindow).__LOGIN_NOTES_TEST__.updates)).toEqual([]);
    await dialog.getByLabel("Телефон", { exact: true }).fill("+15550000456");
    await dialog.getByRole("button", { name: "Заметки", exact: true }).click();
    await expect.poll(() => page.evaluate(() => (window as NotesTestWindow).__LOGIN_NOTES_TEST__.updatedAccounts)).toEqual(["account_synthetic_1"]);
    await dialog.getByRole("button", { name: "Заметки", exact: true }).click();
    await expect(dialog.getByLabel("Телефон", { exact: true })).toHaveValue("+15550000456");
    const started = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.find((call) => call.command === "start_codex_oauth"));
    expect(started?.args.clientKind).toBe(target);
    expect(started?.args.accountId).toBeUndefined();
    await emitTauriEvent(page, "relay-oauth-status", { loginId: "oauth_synthetic", status: "callback_received" });
    await expect(dialog).toHaveCount(0);
    await expect(page.locator(".account-route-selector > button")).toHaveAttribute("data-value", target);
    expect(await page.evaluate(() => (window as NotesTestWindow).__LOGIN_NOTES_TEST__.updatedAccounts)).toEqual(["account_synthetic_1"]);
    const saved = await openNotes(page);
    await expect(saved.getByLabel("Пароль", { exact: true })).toHaveValue(values.password);
    await expect(saved.getByLabel("Телефон", { exact: true })).toHaveValue("+15550000456");
  });
}

test("a route notes load failure leaves stored notes untouched and fields disabled", async ({ page }) => {
  await installNotesMock(page, { loading: "failed", accountRouteKinds: ["siwc"] });
  await page.goto("/");
  await page.getByRole("button", { name: "Подключения", exact: true }).click();
  await page.locator(".account-route-selector > button").click();
  await page.getByRole("option", { name: "Codex · Подключить", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Войти", exact: true });
  await dialog.getByRole("button", { name: "Заметки", exact: true }).click();
  await expect(dialog.getByRole("alert")).toBeVisible();
  await expect(dialog.getByLabel("Пароль", { exact: true })).toBeDisabled();
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  expect(await page.evaluate(() => (window as NotesTestWindow).__LOGIN_NOTES_TEST__.updates)).toEqual([]);
});
