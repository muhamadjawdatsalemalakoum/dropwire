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
    var m = document.getElementById("meta-theme-color");
    if (m) m.content = effectiveTheme() === "dark" ? "#0e1116" : "#f7f9f5";
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

/* =========================================================================
   Lightbox — tap a screenshot to view it full-size. Native <dialog> gives us
   the focus trap, Esc-to-close, and backdrop for free. Progressive: if the
   browser lacks <dialog>.showModal, the screenshots simply stay inline.
   ========================================================================= */
(function () {
  "use strict";
  var lb = document.getElementById("lightbox");
  if (!lb || typeof lb.showModal !== "function") return;
  var lbImg = lb.querySelector(".lightbox-img");
  var closeBtn = lb.querySelector(".lightbox-close");
  var shots = document.querySelectorAll(
    "#see .shot-media img, #see .install-pair img"
  );
  if (!shots.length) return;

  function openShot(img) {
    lbImg.src = img.currentSrc || img.src;
    lbImg.alt = img.alt || "";
    lb.showModal();
    document.documentElement.style.overflow = "hidden"; // lock scroll
    closeBtn.focus();
  }

  Array.prototype.forEach.call(shots, function (img) {
    img.setAttribute("role", "button");
    img.setAttribute("tabindex", "0");
    img.setAttribute("aria-label", "View larger: " + (img.alt || "screenshot"));
    img.addEventListener("click", function () {
      openShot(img);
    });
    img.addEventListener("keydown", function (e) {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        openShot(img);
      }
    });
  });

  closeBtn.addEventListener("click", function () {
    lb.close();
  });
  // Click the dim area (or the image itself) to dismiss.
  lb.addEventListener("click", function (e) {
    if (e.target === lb || e.target === lbImg) lb.close();
  });
  // Fires on Esc (native) and on close() — restore scroll + free the image.
  lb.addEventListener("close", function () {
    document.documentElement.style.overflow = "";
    lbImg.removeAttribute("src");
  });
})();

/* =========================================================================
   Demo video — click-to-load. Nothing from YouTube loads until the visitor
   presses play, so the page stays tracker-free on load. Swaps in the
   privacy-friendly youtube-nocookie player only on click.
   ========================================================================= */
(function () {
  "use strict";
  var wraps = document.querySelectorAll(".video-embed[data-yt]");
  if (!wraps.length) return;
  Array.prototype.forEach.call(wraps, function (wrap) {
    var btn = wrap.querySelector(".video-embed-btn");
    if (!btn) return;
    btn.addEventListener("click", function () {
      var id = wrap.getAttribute("data-yt");
      if (!id) return;
      var iframe = document.createElement("iframe");
      iframe.src =
        "https://www.youtube-nocookie.com/embed/" + id + "?autoplay=1&rel=0";
      iframe.title = "Dropwire demo video";
      iframe.allow =
        "autoplay; encrypted-media; picture-in-picture; web-share; fullscreen";
      iframe.setAttribute("allowfullscreen", "");
      iframe.loading = "lazy";
      wrap.innerHTML = "";
      wrap.appendChild(iframe);
      iframe.focus();
    });
  });
})();
