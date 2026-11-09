import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import authSnapshot from "../../src-tauri/src/local_pool/client_auth_watchdog/auth_snapshot.cjs";
import automaticLogin from "../../src-tauri/src/local_pool/commands/oauth/window/automatic_login.cjs";

const root = new URL("../../src-tauri/src/local_pool/", import.meta.url);
const read = (path) => readFileSync(new URL(path, root), "utf8");
const locales = ["en", "ru"].map((locale) => JSON.parse(read(`auth_page_patterns/${locale}.json`)));

function snapshot(headings, controls, route = "/index.html", hash = "") {
  return authSnapshot(locales, {
    location: { pathname: route, hash },
    document: {
      querySelectorAll: (selector) => (selector === "h1,h2" ? headings : controls)
        .map((textContent) => ({ textContent })),
    },
  });
}

function login({ label = "Continue", url = "https://auth.openai.com/log-in", input = {}, patterns = locales, onInput, attributes = {}, labelledBy = {} } = {}) {
  let submissions = 0;
  class Input {
    type = "email";
    autocomplete = "username";
    name = "email";
    isConnected = true;
    currentValue = "";
    get value() { return this.currentValue; }
    set value(value) { this.currentValue = value; }
    getClientRects() { return [{}]; }
    dispatchEvent() { onInput?.(submitter); }
  }
  const field = new Input();
  const form = {
    action: url, method: "post", target: "",
    querySelectorAll: () => [submitter],
    requestSubmit: () => { submissions++; },
  };
  field.form = form;
  const submitter = {
    textContent: label, form, formTarget: "", isConnected: true,
    getClientRects: () => [{}], hasAttribute: () => false,
    getAttribute: (name) => attributes[name] ?? null,
  };
  const window = {};
  window.top = window.self = window;
  const args = { url, expiresAtMs: Date.now() + 10000, waitUntilMs: Date.now() + 10000, ...input };
  const result = automaticLogin(patterns, args, {
    window, location: new URL(url), URL,
    getComputedStyle: () => ({ visibility: "visible" }),
    HTMLInputElement: Input, Event: class {},
    document: {
      readyState: "complete", querySelector: () => null,
      querySelectorAll: (selector) => selector === "input" ? [field] : [],
      getElementById: (id) => Object.hasOwn(labelledBy, id) ? { textContent: labelledBy[id] } : null,
    },
  });
  return { result, submissions, value: field.value };
}

describe("external login page language resources", () => {
  test("preserves every existing detection and blocked-action pattern", () => {
    expect(locales.map((locale) => locale.loginTitle).join("|"))
      .toBe("sign in to chatgpt|войти в chatgpt|войдите в chatgpt");
    expect(locales.map((locale) => locale.loginPrimaryAction).join("|"))
      .toBe("continue to sign in|sign in with chatgpt|продолжить вход|войти с помощью chatgpt");
    expect(locales.map((locale) => locale.unsafeSubmit).join("|"))
      .toBe("sign.?up|create|reset|forgot|google|apple|microsoft|consent|authorize|созда|сброс|разреш");
  });

  test("recognizes English, Russian and mixed-language login pages", () => {
    for (const title of ["Sign in to ChatGPT", "Войти в ChatGPT", "Войдите в ChatGPT"]) {
      for (const action of ["Continue to sign in", "Sign in with ChatGPT", "Продолжить вход", "Войти с помощью ChatGPT"]) {
        expect(snapshot([title], [action]).loginUiMarkers).toEqual(["login_title", "login_primary_action"]);
      }
    }
    expect(snapshot(["SIGN IN TO CHATGPT"], ["CONTINUE TO SIGN IN"]).loginUiSignal).toBe(true);
  });

  test("retains route normalization and does not invent unsupported language signals", () => {
    expect(snapshot([], [], "/index.html", "#/login/?next=synthetic").route).toBe("/login");
    expect(snapshot(["登录"], ["Continue"]).loginUiMarkers).toEqual([]);
    expect(snapshot(["Sign in to ChatGPT"], ["Continue"]).loginUiMarkers).toEqual(["login_title"]);
  });

  test("keeps safe email detection and submission working", () => {
    expect(login()).toEqual({ result: "email", submissions: 0, value: "" });
    expect(login({ label: "Продолжить", input: { stage: "email", value: "synthetic@example.test" } }))
      .toEqual({ result: "submitted", submissions: 1, value: "synthetic@example.test" });
  });

  test("blocks account creation, reset, external providers and consent in both languages", () => {
    for (const label of ["Sign up", "Create account", "Reset password", "Forgot password", "Google", "Apple", "Microsoft", "Consent", "Authorize", "Создать аккаунт", "Сбросить пароль", "Разрешить доступ"]) {
      expect(login({ label, input: { stage: "email", value: "synthetic@example.test" } }))
        .toEqual({ result: "manual", submissions: 0, value: "" });
    }
  });

  test("login values stay literal data", () => {
    const value = '\"); throw new Error("synthetic input is data"); //';
    expect(login({ input: { stage: "email", value } }))
      .toEqual({ result: "submitted", submissions: 1, value });
  });

  test("rechecks localized safety after filling and fails closed on invalid resources", () => {
    expect(login({ input: { stage: "email", value: "synthetic@example.test" }, onInput: (button) => { button.textContent = "Разрешить доступ"; } }).submissions).toBe(0);
    for (const patterns of [[], [{ unsafeSubmit: "[" }]]) {
      expect(login({ patterns }).result).toBe("manual");
    }
    expect(login({ url: "https://synthetic.example/log-in" }).result).toBe("manual");
    expect(login({ input: { expiresAtMs: 0 } }).result).toBe("manual");
  });

  test("checks accessible submit labels as well as visible button text", () => {
    for (const label of ["Authorize", "Разрешить доступ"]) {
      for (const options of [
        { attributes: { "aria-label": label } },
        { attributes: { "aria-labelledby": "missing label" }, labelledBy: { label } },
      ]) {
        expect(login({ ...options, input: { stage: "email", value: "synthetic@example.test" } }))
          .toEqual({ result: "manual", submissions: 0, value: "" });
      }
    }
  });
});
