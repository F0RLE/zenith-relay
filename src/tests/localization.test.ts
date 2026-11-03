import { describe, expect, test } from "bun:test";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { runInNewContext } from "node:vm";
import { createInstance } from "i18next";
import { createLanguageController, readLanguagePreference } from "../src/i18n/controller";
import { canonicalLocale, languageChain, locales, resolveLanguage, type LocaleDefinition } from "../src/i18n/registry";
import { loadTranslations } from "../src/i18n/resources";
import { flattenTranslations, validateRegistry, validateTranslations } from "../src/i18n/contract";
import { synchronizeNativeLanguage } from "../src/i18n/presentation";

const definition = (code: string, fallbacks = ["en"]): LocaleDefinition => ({ code, nativeName: code, englishName: code, direction: "ltr", format: "json", helpLocale: "en", fallbacks });

describe("scalable Desktop localization", () => {
  test("every registered dictionary and locale-aware plural set is valid", async () => {
    validateRegistry(locales);
    const english = await loadTranslations("en");
    for (const locale of locales) {
      expect(existsSync(new URL(`../../docs/help/${locale.helpLocale}/README.md`, import.meta.url))).toBe(true);
      expect(validateTranslations(locale.code, await loadTranslations(locale.code), english, true).missing).toEqual([]);
    }
    const files = readdirSync(join(import.meta.dir, "../src/i18n/locales")).filter(file => file !== "legacyAutomationNames.json");
    expect(files.sort()).toEqual(locales.map(locale => `${locale.code}.${locale.format}`).sort());
  });

  test("preserves scripts, regions and canonical OS spelling", () => {
    const available = [definition("en", []), definition("pt"), definition("pt-BR", ["pt"]), definition("zh-Hans"), definition("zh-Hant"), definition("sr-Latn")];
    expect(canonicalLocale("pt_BR.UTF-8")).toBe("pt-BR");
    expect(resolveLanguage("pt-BR", available)).toBe("pt-BR");
    expect(resolveLanguage("pt-PT", available)).toBe("pt");
    expect(resolveLanguage("zh-Hant-TW", available)).toBe("zh-Hant");
    expect(resolveLanguage("zh-Hans", available)).toBe("zh-Hans");
    expect(resolveLanguage("sr-Latn-RS", available)).toBe("sr-Latn");
    expect(resolveLanguage("zh", available)).toBe("en");
    expect(resolveLanguage("../../path", available)).toBe("en");
    expect(languageChain("pt-BR", available)).toEqual(["pt-BR", "pt", "en"]);
  });

  test("150+ metadata records do not load their dictionaries", async () => {
    const available = [definition("en", []), ...Array.from({ length: 160 }, (_, index) => definition(`en-x-test-${index}`))];
    validateRegistry(available);
    const reads: string[] = [];
    const engine = createInstance();
    const controller = createLanguageController(engine, async code => { reads.push(code); return { example: code }; }, available);
    await controller.initialize("en-x-test-159");
    expect(reads.sort()).toEqual(["en", "en-x-test-159"]);
    for (let index = 0; index < 10; index++) await controller.change(`en-x-test-${index}`, false);
    expect(available.filter(locale => engine.hasResourceBundle(locale.code, "translation")).length).toBeLessThanOrEqual(4);
  });

  test("latest selection wins even if an earlier dictionary finishes last", async () => {
    let release!: () => void;
    const wait = new Promise<void>(resolve => { release = resolve; });
    const engine = createInstance();
    const controller = createLanguageController(engine, async code => { if (code === "ru") await wait; return { example: code }; });
    await controller.initialize("en");
    const slow = controller.change("ru", false);
    await controller.change("en", false);
    release();
    expect(await slow).toBe(false);
    expect(engine.language).toBe("en");
  });

  test("missing keys fall back, failed loads retain current language and retry", async () => {
    let failures = 1;
    const engine = createInstance();
    const controller = createLanguageController(engine, async code => {
      if (code === "ru" && failures-- > 0) throw new Error("synthetic failed chunk");
      return code === "en" ? { example: "English", fallback: "Available" } : { example: "Русский" };
    });
    await controller.initialize("en");
    await expect(controller.change("ru", false)).rejects.toThrow("synthetic");
    expect(engine.language).toBe("en");
    await controller.change("ru", false);
    expect(engine.t("example")).toBe("Русский");
    expect(engine.t("fallback")).toBe("Available");
  });

  test("invalid dictionaries and fallback cycles fail the quality gate", () => {
    expect(() => validateTranslations("ru", { name: "Привет {{other}}" }, { name: "Hi {{name}}" }, false)).toThrow("interpolation");
    expect(() => validateTranslations("en", { missing: "value" }, { name: "name" }, false)).toThrow("unknown key");
    expect(() => flattenTranslations({ empty: "" })).toThrow("Empty");
    expect(() => validateRegistry([definition("en", ["ru"]), definition("ru", ["en"])])).toThrow("Cyclic");
  });

  test("unavailable preference storage does not block initialization or selection", async () => {
    const previous = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
    Object.defineProperty(globalThis, "localStorage", { configurable: true, value: { getItem() { throw new Error("synthetic denial"); }, setItem() { throw new Error("synthetic denial"); } } });
    try {
      expect(readLanguagePreference()).toBeNull();
      const engine = createInstance();
      const controller = createLanguageController(engine, async code => ({ example: code }));
      await controller.initialize("en");
      expect(await controller.change("ru")).toBe(true);
      expect(engine.language).toBe("ru");
    } finally {
      if (previous) Object.defineProperty(globalThis, "localStorage", previous);
      else Reflect.deleteProperty(globalThis, "localStorage");
    }
  });

  test("a failed selected startup dictionary uses the English recovery UI", async () => {
    const engine = createInstance();
    const controller = createLanguageController(engine, async code => {
      if (code === "ru") throw new Error("synthetic missing chunk");
      return { example: "English" };
    });
    await controller.initialize("ru");
    expect(engine.language).toBe("en");
    expect(engine.t("example")).toBe("English");
  });

  test("native language synchronization recovers after a failed preference write", async () => {
    const applied: string[] = [];
    await expect(synchronizeNativeLanguage(async () => {
      applied.push("ru");
      throw new Error("synthetic preference write failure");
    })).rejects.toThrow("preference write failure");
    await synchronizeNativeLanguage(async () => { applied.push("en"); });
    expect(applied).toEqual(["ru", "en"]);
  });
});

const startupScript = readFileSync(new URL("../public/startup.js", import.meta.url), "utf8");
const startupCopy = (label: string) => ({ interface: `${label} interface`, runtime: `${label} runtime`, slow: `${label} slow`, error: `${label} error`, reload: `${label} reload` });

function startupHarness(language: string, cached: unknown, preference?: string) {
  const storage = new Map([["relay.startupCopy", JSON.stringify(cached)]]);
  if (preference) storage.set("relay.language", preference);
  const html = { dataset: {}, lang: "", dir: "" };
  const message = { textContent: "" };
  const reload = { hidden: true, textContent: "", addEventListener() {} };
  const listeners = new Map<string, () => void>();
  let localeChanged = () => {};
  runInNewContext(startupScript, {
    performance: { mark() {} }, navigator: { language },
    localStorage: { getItem: (key: string) => storage.get(key) ?? null },
    document: {
      documentElement: html,
      getElementById: (id: string) => id === "splash-message" ? message : reload,
      addEventListener: (name: string, listener: () => void) => listeners.set(name, listener),
    },
    window: { setTimeout() { return 1; }, addEventListener() {} },
    MutationObserver: class {
      constructor(listener: () => void) { localeChanged = listener; }
      observe() {}
    },
  });
  listeners.get("DOMContentLoaded")!();
  return { html, message, reload, storage, localeChanged: () => localeChanged() };
}

describe("pre-bundle language cache", () => {
  test("regional and OS locales reuse the resolved base dictionary", () => {
    for (const language of ["ru-RU", "ru_RU.UTF-8"]) {
      const startup = startupHarness(language, { locale: "ru", direction: "ltr", copy: startupCopy("Russian") });
      expect(startup.message.textContent).toBe("Russian interface");
      expect(startup.reload.textContent).toBe("Russian reload");
    }
  });

  test("explicit selection takes precedence over the system locale", () => {
    const startup = startupHarness("ru-RU", { locale: "ru", direction: "ltr", copy: startupCopy("Russian") }, "en");
    expect(startup.message.textContent).toBe("Preparing the interface…");
  });

  test("only explicit aliases may select a different script", () => {
    const cache = { locale: "zh-Hant", aliases: ["zh-TW"], direction: "ltr", copy: startupCopy("Traditional") };
    expect(startupHarness("zh-TW", cache).message.textContent).toBe("Traditional interface");
    expect(startupHarness("zh-Hans", cache).message.textContent).toBe("Preparing the interface…");
  });

  test("language changes read the refreshed text and direction", () => {
    const startup = startupHarness("en", { locale: "en", direction: "ltr", copy: startupCopy("English") });
    startup.storage.set("relay.startupCopy", JSON.stringify({ locale: "ar", direction: "rtl", copy: startupCopy("Arabic") }));
    startup.html.lang = "ar";
    startup.localeChanged();
    expect(startup.message.textContent).toBe("Arabic interface");
    expect(startup.reload.textContent).toBe("Arabic reload");
    expect(startup.html.dir).toBe("rtl");
  });

  test("corrupt or oversized optional text retains the English fallback", () => {
    for (const cached of [null, { locale: null, aliases: ["en"], direction: "rtl", copy: startupCopy("Invalid") }, { locale: "en", direction: "rtl", copy: { ...startupCopy("Invalid"), reload: {} } }, { locale: "en", direction: "ltr", copy: { ...startupCopy("Invalid"), error: "x".repeat(1024) } }]) {
      const startup = startupHarness("en", cached);
      expect(startup.message.textContent).toBe("Preparing the interface…");
      expect(startup.html.dir).toBe("ltr");
    }
  });
});
