(function () {
  performance.mark("zenith:html-start");
  var html = document.documentElement;
  var theme = "system";
  var language = navigator.language;
  try {
    var storedTheme = localStorage.getItem("relay.theme");
    if (storedTheme === "light" || storedTheme === "dark") theme = storedTheme;
    language = localStorage.getItem("relay.language") || language;
  } catch (_) {
    // Storage can be unavailable on the first WebView load.
  }
  html.dataset.theme = theme;
  html.lang = language.toLowerCase().startsWith("ru") ? "ru" : "en";

  var phase = "interface";
  var slow = false;
  var finished = false;
  var copy = {
    en: {
      interface: "Preparing the interface…",
      runtime: "Loading your workspace…",
      slow: "Startup is taking longer than usual",
      error: "Relay could not start",
      reload: "Reload",
    },
    ru: {
      interface: "Подготовка интерфейса…",
      runtime: "Загрузка рабочего пространства…",
      slow: "Запуск занимает больше времени",
      error: "Не удалось запустить Relay",
      reload: "Перезагрузить",
    },
  };

  function render() {
    if (finished) return;
    html.dataset.startupState = phase;
    var message = document.getElementById("splash-message");
    if (!message) return;
    var text = copy[html.lang.startsWith("ru") ? "ru" : "en"];
    var failed = phase === "error";
    message.textContent = failed ? text.error : slow ? text.slow : text[phase];
    var reload = document.getElementById("splash-reload");
    reload.hidden = !failed && !slow;
    reload.textContent = text.reload;
  }

  // This watchdog runs before the entry bundle: a failed import must not leave a blank window.
  var slowTimer = window.setTimeout(function () {
    slow = true;
    render();
  }, 8_000);
  var localeObserver = new MutationObserver(render);
  localeObserver.observe(html, { attributes: true, attributeFilter: ["lang"] });
  window.addEventListener("zenith-startup-phase", function (event) {
    if (event.detail !== "runtime" && event.detail !== "error") return;
    phase = event.detail;
    render();
  });
  window.addEventListener("error", function (event) {
    if (event.target instanceof HTMLScriptElement || event.error) {
      phase = "error";
      render();
    }
  }, true);
  window.addEventListener("zenith-startup-ready", function () {
    finished = true;
    window.clearTimeout(slowTimer);
    localeObserver.disconnect();
    document.getElementById("root").inert = false;
    html.dataset.startupReady = "true";
    var splash = document.getElementById("splash-screen");
    if (!splash) return;
    // Do not depend on transitionend: background windows can skip animation frames.
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) splash.remove();
    else window.setTimeout(function () { splash.remove(); }, 150);
  }, { once: true });
  document.addEventListener("DOMContentLoaded", function () {
    render();
    var reload = document.getElementById("splash-reload");
    if (reload) reload.addEventListener("click", function () {
      window.location.reload();
    });
  }, { once: true });
})();
