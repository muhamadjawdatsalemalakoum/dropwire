/* =========================================================================
   Dropwire — small, dependency-free interactions.
   Only job: a light/dark theme toggle that respects the OS preference by
   default and remembers an explicit user choice. No trackers, no analytics.
   ========================================================================= */
(function () {
  "use strict";

  var STORAGE_KEY = "dropwire-theme";
  var root = document.documentElement;
  var toggle = document.getElementById("theme-toggle");
  if (!toggle) return;

  var mql = window.matchMedia
    ? window.matchMedia("(prefers-color-scheme: dark)")
    : null;

  // Resolve what the user is *currently* seeing.
  function effectiveTheme() {
    var explicit = root.getAttribute("data-theme");
    if (explicit === "light" || explicit === "dark") return explicit;
    return mql && mql.matches ? "dark" : "light";
  }

  // Keep the button's a11y state + label in sync with what's on screen.
  function syncToggle() {
    var isDark = effectiveTheme() === "dark";
    toggle.setAttribute("aria-pressed", String(isDark));
    toggle.setAttribute(
      "aria-label",
      isDark ? "Switch to light theme" : "Switch to dark theme"
    );
    toggle.title = isDark ? "Switch to light theme" : "Switch to dark theme";
  }

  toggle.addEventListener("click", function () {
    var next = effectiveTheme() === "dark" ? "light" : "dark";
    root.setAttribute("data-theme", next);
    try {
      localStorage.setItem(STORAGE_KEY, next);
    } catch (e) {
      /* storage may be unavailable (private mode) — toggle still works for the session */
    }
    syncToggle();
  });

  // If the user has no explicit choice, follow OS changes live.
  if (mql) {
    var onChange = function () {
      var saved;
      try {
        saved = localStorage.getItem(STORAGE_KEY);
      } catch (e) {
        saved = null;
      }
      if (saved !== "light" && saved !== "dark") syncToggle();
    };
    if (mql.addEventListener) mql.addEventListener("change", onChange);
    else if (mql.addListener) mql.addListener(onChange);
  }

  syncToggle();
})();
