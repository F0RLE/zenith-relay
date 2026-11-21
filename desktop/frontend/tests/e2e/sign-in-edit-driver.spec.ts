import { expect, test } from "../bun-playwright";
import { fileURLToPath } from "node:url";

type Menu = { token: string; canCopy: boolean; canCut: boolean; canPaste: boolean; canSelectAll: boolean };
type DriverWindow = Window & {
  menus: Menu[];
  __relaySignInEdit: (request: { token: string; action: string; value?: string }) => { ok: boolean; text?: string };
};
test.beforeEach(async ({ page }) => {
  await page.route("https://sign-in.example.test/**", (route) => route.fulfill({ contentType: "text/html", body: '<label>Email<input id=field></label><div contenteditable=true id=editable>synthetic editable text</div>' }));
  await page.addInitScript(() => {
    const menus: Menu[] = [];
    Object.assign(window, { menus, ipc: { postMessage: (raw: string) => menus.push(JSON.parse(raw)) } });
  });
  await page.addInitScript({ path: fileURLToPath(new URL("../../../src-tauri/src/local_pool/commands/oauth/window/embedded/edit_menu.js", import.meta.url)) });
  await page.goto("https://sign-in.example.test/");
});

for (const type of ["text", "password", "email"]) {
  test(`sign-in menu edits ${type} without sending field contents in notifications`, async ({ page }) => {
    const field = page.locator("#field");
    await field.evaluate((input, type) => input.setAttribute("type", type), type);
    await field.fill("synthetic@example.test");
    await field.press("ControlOrMeta+A");
    await field.click({ button: "right" });
    const menu = await page.evaluate(() => (window as unknown as DriverWindow).menus.at(-1)!);
    expect(menu).toMatchObject({ canCopy: true, canCut: true, canPaste: true, canSelectAll: true });
    expect(JSON.stringify(menu)).not.toContain("synthetic@example.test");
    const selection = await page.evaluate((token) => (window as unknown as DriverWindow).__relaySignInEdit({ token, action: "read" }), menu.token);
    expect(selection).toEqual({ ok: true, text: "synthetic@example.test" });
    expect(await page.evaluate((token) => (window as unknown as DriverWindow).__relaySignInEdit({ token, action: "paste", value: "replacement@example.test" }), menu.token)).toEqual({ ok: true });
    await expect(field).toHaveValue("replacement@example.test");
    expect(await page.evaluate((token) => (window as unknown as DriverWindow).__relaySignInEdit({ token, action: "cut" }), menu.token)).toEqual({ ok: false });
  });
}

test("changed fields and replaced menus cannot mutate a stale selection", async ({ page }) => {
  const field = page.locator("#field");
  await field.fill("first value");
  await field.press("ControlOrMeta+A");
  await field.click({ button: "right" });
  const token = await page.evaluate(() => (window as unknown as DriverWindow).menus.at(-1)!.token);
  await field.fill("new value");
  expect(await page.evaluate((token) => (window as unknown as DriverWindow).__relaySignInEdit({ token, action: "cut" }), token)).toEqual({ ok: false });
  await expect(field).toHaveValue("new value");
  await field.click({ button: "right" });
  expect(await page.evaluate((token) => (window as unknown as DriverWindow).__relaySignInEdit({ token, action: "paste", value: "stale" }), token)).toEqual({ ok: false });
});

test("read-only fields expose copy and select-all but no write actions", async ({ page }) => {
  const field = page.locator("#field");
  await field.fill("read-only value");
  await field.evaluate((input: HTMLInputElement) => { input.readOnly = true; input.select(); });
  await field.click({ button: "right" });
  const menu = await page.evaluate(() => (window as unknown as DriverWindow).menus.at(-1)!);
  expect(menu).toMatchObject({ canCopy: true, canCut: false, canPaste: false });
  expect(await page.evaluate((token) => (window as unknown as DriverWindow).__relaySignInEdit({ token, action: "paste", value: "changed" }), menu.token)).toEqual({ ok: false });
  await expect(field).toHaveValue("read-only value");
});

test("synthetic context events cannot open the launcher menu", async ({ page }) => {
  await page.locator("#field").dispatchEvent("contextmenu");
  expect(await page.evaluate(() => (window as unknown as DriverWindow).menus)).toEqual([]);
});

for (const initial of ["synthetic editable text", ""]) {
  test(`contenteditable can paste into ${initial ? "a selection" : "an empty field"}`, async ({ page }) => {
    const field = page.locator("#editable");
    await field.fill(initial);
    await field.press("ControlOrMeta+A");
    await field.click({ button: "right" });
    const menu = await page.evaluate(() => (window as unknown as DriverWindow).menus.at(-1)!);
    expect(menu.canPaste).toBe(true);
    expect(await page.evaluate((token) => (window as unknown as DriverWindow).__relaySignInEdit({ token, action: "paste", value: "replacement text" }), menu.token)).toEqual({ ok: true });
    await expect(field).toHaveText("replacement text");
    await field.press("ControlOrMeta+A");
    await field.click({ button: "right" });
    const next = await page.evaluate(() => (window as unknown as DriverWindow).menus.at(-1)!);
    expect(await page.evaluate((token) => (window as unknown as DriverWindow).__relaySignInEdit({ token, action: "cut" }), next.token)).toEqual({ ok: true });
    await expect(field).toHaveText("");
  });
}
