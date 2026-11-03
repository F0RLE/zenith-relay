function automaticLogin(input) {
  try {
    const fresh = () => Number.isFinite(input.expiresAtMs) && Date.now() < input.expiresAtMs;
    if (!fresh()) return "manual";
    if (window.top !== window.self || location.origin !== "https://auth.openai.com" || location.href !== input.url) return "manual";
    const passwordPage = ["/log-in", "/log-in/password", "/u/login/identifier", "/u/login/password"].includes(location.pathname);
    const totpPage = ["/mfa/totp", "/u/mfa-otp-challenge"].includes(location.pathname);
    if (!passwordPage && !totpPage) return "manual";
    if (document.readyState === "loading") return "wait";
    const visible = (element) => element.getClientRects().length > 0 && getComputedStyle(element).visibility !== "hidden";
    if (document.querySelector('[data-sitekey], iframe[src*="captcha"], iframe[src*="turnstile"], input[autocomplete="new-password"]')
      || [...document.querySelectorAll('[role="alert"], [aria-invalid="true"]')].some((element) => visible(element) && (element.textContent.trim() || element.matches("input")))) return "manual";
    const fields = [...document.querySelectorAll("input")].filter((field) => visible(field) && !field.disabled && !field.readOnly);
    if (!fields.length && Date.now() < input.waitUntilMs) return "wait";
    const passwords = fields.filter((field) => field.type === "password" && field.autocomplete !== "new-password");
    const emails = fields.filter((field) => field.type === "email" || field.autocomplete === "username" || field.name === "username" || field.name === "email");
    const codes = fields.filter((field) => field.autocomplete === "one-time-code" || field.name === "code" || field.name === "otp");
    if (passwords.length > 1 || emails.length > 1 || codes.length > 1) return "manual";
    let field, stage;
    if (totpPage && codes.length === 1 && !passwords.length && !emails.length) { field = codes[0]; stage = "totp"; }
    else if (passwordPage && !codes.length && passwords.length === 1) { field = passwords[0]; stage = "password"; }
    else if (passwordPage && !codes.length && emails.length === 1) { field = emails[0]; stage = "email"; }
    else return "manual";
    const form = field.form;
    if (!form) return "manual";
    const submitters = [...form.querySelectorAll('button:not([type]), button[type="submit"], input[type="submit"]')].filter((button) => visible(button) && !button.disabled);
    if (submitters.length !== 1) return "manual";
    const submitter = submitters[0];
    const safeForm = () => new URL(form.action, location.href).origin === location.origin
      && form.method.toLowerCase() === "post" && (!form.target || form.target === "_self")
      && !(submitter.hasAttribute("formaction") && new URL(submitter.formAction, location.href).origin !== location.origin)
      && !(submitter.hasAttribute("formmethod") && submitter.formMethod.toLowerCase() !== "post")
      && (!submitter.formTarget || submitter.formTarget === "_self")
      && !/sign.?up|create|reset|forgot|google|apple|microsoft|consent|authorize|созда|сброс|разреш/i.test(submitter.textContent);
    if (!safeForm()) return "manual";
    if (!input.stage) return stage;
    if (input.stage !== stage || typeof input.value !== "string") return "manual";
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set;
    const fill = (target, value) => {
      if (!fresh()) return false;
      setter.call(target, value);
      if (target.value !== value) return false;
      target.dispatchEvent(new Event("input", { bubbles: true }));
      target.dispatchEvent(new Event("change", { bubbles: true }));
      return target.isConnected && target.form === form && target.value === value;
    };
    if (stage === "password") {
      const username = form.querySelector('input[name="username"], input[name="email"]');
      if (username && !visible(username) && username.value && username.value.toLowerCase() !== input.email?.toLowerCase()) return "manual";
      if (emails.length && (typeof input.email !== "string" || emails[0].form !== form || !fill(emails[0], input.email))) return "manual";
    }
    if (!fill(field, input.value)) return "manual";
    if (!fresh() || location.href !== input.url || !safeForm() || !submitter.isConnected || submitter.form !== form) return "manual";
    form.requestSubmit(submitter);
    return "submitted";
  } catch {
    return "manual";
  }
}
