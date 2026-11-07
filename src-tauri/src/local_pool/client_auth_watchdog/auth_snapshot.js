function authSnapshot(locales) {
  const normalize = (value) => String(value || "")
    .replace(/^#/, "").split(/[?#]/, 1)[0].replace(/\/+$/, "") || "/";
  const path = normalize(location.pathname);
  const hash = normalize(location.hash);
  const route = hash !== "/" ? hash : path;
  const textOf = (selector) => Array.from(document.querySelectorAll(selector))
    .map((node) => String(node.textContent || "").replace(/\s+/g, " ").trim().slice(0, 160))
    .filter(Boolean);
  const headings = textOf("h1,h2").join(" ");
  const controls = textOf("button,a").join(" ");
  const matches = (key, text) => locales.some((locale) => new RegExp(locale[key], "i").test(text));
  const markers = [];
  if (matches("loginTitle", headings)) markers.push("login_title");
  if (matches("loginPrimaryAction", controls)) markers.push("login_primary_action");
  return {
    route: route.slice(0, 160),
    loginUiSignal: markers.length > 0,
    loginUiMarkers: markers,
  };
}
