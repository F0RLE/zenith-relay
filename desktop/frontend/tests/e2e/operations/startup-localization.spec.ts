import { expect, test, type Page } from "@playwright/test";

const startupCopy = (label: string) => ({ interface: `${label} interface`, runtime: `${label} runtime`, slow: `${label} slow`, error: `${label} error`, reload: `${label} reload` });

async function showStartup(page: Page, language: string, cached: unknown, preference?: string) {
  await page.clock.install();
  await page.route(/\/assets\/[^/]+\.js$/, (route) => route.fulfill({ contentType: "text/javascript", body: "" }));
  await page.addInitScript(({ language, cached, preference }) => {
    Object.defineProperty(navigator, "language", { configurable: true, value: language });
    localStorage.setItem("relay.startupCopy", JSON.stringify(cached));
    if (preference) localStorage.setItem("relay.language", preference);
    else localStorage.removeItem("relay.language");
  }, { language, cached, preference });
  await page.goto("/", { waitUntil: "domcontentloaded" });
}

for (const language of ["ru-RU", "ru_RU.UTF-8"]) {
  test(`pre-bundle startup resolves the regional locale ${language}`, async ({ page }) => {
    await showStartup(page, language, { locale: "ru", direction: "ltr", copy: startupCopy("Russian") });
    await expect(page.locator("#splash-message")).toHaveText("Russian interface");
    await expect(page.locator("#splash-reload")).toHaveText("Russian reload");
  });
}

test("pre-bundle startup prefers explicit selection over the system locale", async ({ page }) => {
  await showStartup(page, "ru-RU", { locale: "ru", direction: "ltr", copy: startupCopy("Russian") }, "en");
  await expect(page.locator("#splash-message")).toHaveText("Preparing the interface…");
});

for (const language of ["zh-TW", "zh-Hans"]) {
  test(`pre-bundle startup only accepts explicit script aliases for ${language}`, async ({ page }) => {
    await showStartup(page, language, { locale: "zh-Hant", aliases: ["zh-TW"], direction: "ltr", copy: startupCopy("Traditional") });
    await expect(page.locator("#splash-message")).toHaveText(language === "zh-TW" ? "Traditional interface" : "Preparing the interface…");
  });
}

test("pre-bundle startup observes refreshed text and writing direction", async ({ page }) => {
  await showStartup(page, "en", { locale: "en", direction: "ltr", copy: startupCopy("English") });
  await expect(page.locator("#splash-message")).toHaveText("English interface");
  await page.evaluate((copy) => {
    localStorage.setItem("relay.startupCopy", JSON.stringify({ locale: "ar", direction: "rtl", copy }));
    document.documentElement.lang = "ar";
  }, startupCopy("Arabic"));
  await expect(page.locator("#splash-message")).toHaveText("Arabic interface");
  await expect(page.locator("#splash-reload")).toHaveText("Arabic reload");
  await expect(page.locator("html")).toHaveAttribute("dir", "rtl");
});

for (const [name, cached] of [
  ["missing", null],
  ["invalid locale", { locale: null, aliases: ["en"], direction: "rtl", copy: startupCopy("Invalid") }],
  ["invalid text", { locale: "en", direction: "rtl", copy: { ...startupCopy("Invalid"), reload: {} } }],
  ["oversized text", { locale: "en", direction: "ltr", copy: { ...startupCopy("Invalid"), error: "x".repeat(1024) } }],
] as const) {
  test(`pre-bundle startup retains English for ${name}`, async ({ page }) => {
    await showStartup(page, "en", cached);
    await expect(page.locator("#splash-message")).toHaveText("Preparing the interface…");
    await expect(page.locator("html")).toHaveAttribute("dir", "ltr");
  });
}
