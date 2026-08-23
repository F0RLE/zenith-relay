(function () {
  performance.mark("zenith:html-start");
  var theme = "system";
  try {
    var storedTheme = localStorage.getItem("relay.theme");
    if (storedTheme === "light" || storedTheme === "dark") theme = storedTheme;
  } catch (_) {
    // Keep the system theme when storage is unavailable.
  }
  document.documentElement.dataset.theme = theme;
})();
