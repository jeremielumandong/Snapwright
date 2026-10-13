// Snapwright site: Omarchy themes. Loaded in <head> so the saved theme applies before the
// page paints. The palette mapping matches the app's (crates/app/src/omarchy.rs) and the
// design handoff; the app screenshots come from scripts/site-shots.sh, one folder per theme.
(function () {
  "use strict";

  var KEY = "snapwright-theme";
  // The original Snapwright look: the tokens in style.css.
  var SNAP = { id: "snapwright", name: "Snapwright", mode: "light", original: true,
    accent: "#2f6fd6", red: "#d93a3f", green: "#30a46c", yellow: "#e5a10a", background: "#f4f1ea", bright_foreground: "#17160f" };
  function T(id, name, mode, accent, selection, background, darkBg, darkerBg, lighterBg, fg, bright, red, yellow, green) {
    return { id: id, name: name, mode: mode, accent: accent, selection: selection, background: background,
      dark_background: darkBg, darker_background: darkerBg, lighter_background: lighterBg,
      foreground: fg, bright_foreground: bright, red: red, yellow: yellow, green: green };
  }
  // From each theme's colors.toml in basecamp/omarchy; the same list as the app's BUNDLED.
  var THEMES = [SNAP,
    T("tokyo-night", "Tokyo Night", "dark", "#7aa2f7", "#292e42", "#1a1b26", "#13141c", "#0e0e14", "#24283b", "#a9b1d6", "#c0caf5", "#f7768e", "#e0af68", "#9ece6a"),
    T("catppuccin", "Catppuccin", "dark", "#89b4fa", "#45475a", "#1e1e2e", "#161622", "#101019", "#313244", "#cdd6f4", "#cdd6f4", "#f38ba8", "#f9e2af", "#a6e3a1"),
    T("catppuccin-latte", "Catppuccin Latte", "light", "#1e66f5", "#ccd0da", "#eff1f5", "#e3e4e8", "#d7d8dc", "#dce0e8", "#4c4f69", "#4c4f69", "#d20f39", "#df8e1d", "#40a02b"),
    T("ethereal", "Ethereal", "dark", "#7d82d9", "#252e56", "#060b1e", "#040816", "#030610", "#131a3a", "#ffcead", "#ffcead", "#ed5b5a", "#e9bb4f", "#92a593"),
    T("everforest", "Everforest", "dark", "#7fbbb3", "#3d484d", "#2d353b", "#21272c", "#181d20", "#343f44", "#d3c6aa", "#d3c6aa", "#e67e80", "#dbbc7f", "#a7c080"),
    T("flexoki-light", "Flexoki Light", "light", "#205ea6", "#cecdc3", "#fffcf0", "#f2efe4", "#e5e2d8", "#e6e4d9", "#100f0f", "#100f0f", "#d14d41", "#d0a215", "#879a39"),
    T("gruvbox", "Gruvbox", "dark", "#7daea3", "#504945", "#282828", "#1e1e1e", "#161616", "#3c3836", "#d4be98", "#d4be98", "#ea6962", "#d8a657", "#a9b665"),
    T("hackerman", "Hackerman", "dark", "#82fb9c", "#1f253a", "#0b0c16", "#080910", "#06060c", "#151828", "#ddf7ff", "#ddf7ff", "#50f872", "#50f7d4", "#4fe88f"),
    T("kanagawa", "Kanagawa", "dark", "#dcd7ba", "#363646", "#1f1f28", "#17171e", "#111116", "#223249", "#dcd7ba", "#dcd7ba", "#c34043", "#c0a36e", "#76946a"),
    T("lumon", "Lumon", "dark", "#8bc9eb", "#243d56", "#16242d", "#101b21", "#0b1216", "#1b2d40", "#d6e2ee", "#f2fcff", "#4d86b0", "#6fa4c9", "#5e95bc"),
    T("matte-black", "Matte Black", "dark", "#e68e0d", "#2a2a2a", "#121212", "#0d0d0d", "#090909", "#1e1e1e", "#bebebe", "#bebebe", "#d35f5f", "#b91c1c", "#ffc107"),
    T("nord", "Nord", "dark", "#81a1c1", "#434c5e", "#2e3440", "#222730", "#191c23", "#3b4252", "#d8dee9", "#d8dee9", "#bf616a", "#ebcb8b", "#a3be8c"),
    T("osaka-jade", "Osaka Jade", "dark", "#509475", "#32473b", "#111c18", "#0c1512", "#090f0d", "#23372b", "#c1c497", "#f7e8b2", "#ff5345", "#459451", "#549e6a"),
    T("ristretto", "Ristretto", "dark", "#f38d70", "#403e41", "#2c2525", "#211b1b", "#181414", "#3d2f2a", "#e6d9db", "#e6d9db", "#fd6883", "#f9cc6c", "#adda78"),
    T("rose-pine", "Rosé Pine", "light", "#56949f", "#dfdad9", "#faf4ed", "#ede7e1", "#e1dbd5", "#f2e9e1", "#575279", "#575279", "#b4637a", "#ea9d34", "#286983"),
    T("white", "White", "light", "#6e6e6e", "#c0c0c0", "#ffffff", "#f5f5f5", "#e8e8e8", "#c0c0c0", "#000000", "#000000", "#2a2a2a", "#4a4a4a", "#3a3a3a")
  ];

  function rgb(h) { h = h.replace("#", ""); return [0, 2, 4].map(function (i) { return parseInt(h.slice(i, i + 2), 16); }); }
  function hex(a) { return "#" + a.map(function (v) { return Math.round(Math.max(0, Math.min(255, v))).toString(16).padStart(2, "0"); }).join(""); }
  function mix(a, b, t) { var x = rgb(a), y = rgb(b); return hex(x.map(function (v, i) { return v + (y[i] - v) * t; })); }
  function alpha(h, a) { var c = rgb(h); return "rgba(" + c[0] + "," + c[1] + "," + c[2] + "," + a + ")"; }
  function lum(h) {
    var c = rgb(h).map(function (v) { v /= 255; return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4); });
    return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
  }

  // Omarchy colors.toml → site tokens. The original theme keeps style.css as it is.
  function varsFor(t) {
    if (t.original) return {};
    var dark = t.mode === "dark";
    var bg = t.background, ink = t.bright_foreground, fg = t.foreground;
    var onAccent = lum(t.accent) > 0.4 ? t.darker_background : "#ffffff";
    var nightBg = dark ? t.darker_background : mix(ink, "#000000", 0.1);
    var nightHead = dark ? ink : bg;
    var nightText = dark ? fg : mix(bg, ink, 0.18);
    return {
      "--paper": bg, "--paper-2": dark ? t.lighter_background : mix(bg, "#ffffff", 0.55), "--sand": t.selection,
      "--rule": mix(bg, ink, dark ? 0.2 : 0.22), "--ink": ink, "--ink-2": mix(ink, bg, 0.2), "--muted": mix(fg, bg, 0.35),
      "--blue": t.accent, "--blue-2": mix(t.accent, dark ? "#ffffff" : "#000000", 0.12), "--blue-3": mix(t.accent, "#ffffff", 0.35),
      "--red": t.red, "--on-accent": onAccent, "--header-bg": alpha(bg, 0.92),
      "--night-bg": nightBg, "--night-head": nightHead, "--night-text": nightText, "--night-muted": mix(nightText, nightBg, 0.3),
      "--night-rule": mix(nightBg, nightText, 0.2), "--pre-bg": mix(nightBg, "#000000", 0.3), "--pre-fg": nightText,
      "--pre-dim": mix(nightText, nightBg, 0.45),
      "--dock-bg": dark ? alpha(t.darker_background, 0.96) : alpha(mix(ink, "#000000", 0.2), 0.95),
      "--dock-line": dark ? alpha(ink, 0.14) : "rgba(255,255,255,.12)",
      "--dock-fg": dark ? ink : "#ffffff",
      "--dock-fg3": dark ? mix(ink, t.darker_background, 0.45) : "rgba(255,255,255,.6)"
    };
  }

  function find(id) {
    for (var i = 0; i < THEMES.length; i++) if (THEMES[i].id === id) return THEMES[i];
    return null;
  }
  function saved() {
    try {
      var q = /[?&]theme=([\w-]+)/.exec(location.search);
      return find((q && q[1]) || localStorage.getItem(KEY)) || SNAP;
    } catch (e) { return SNAP; }
  }

  var current = saved();
  var applied = [];
  var listeners = [];

  function apply(t) {
    var root = document.documentElement.style;
    applied.forEach(function (k) { root.removeProperty(k); });
    var v = varsFor(t);
    applied = Object.keys(v);
    applied.forEach(function (k) { root.setProperty(k, v[k]); });
    document.documentElement.setAttribute("data-theme", t.id);
    var shot = function (name) { return "img/themes/" + t.id + "/" + name + ".png"; };
    Array.prototype.forEach.call(document.querySelectorAll("img[data-shot]"), function (img) {
      img.src = shot(img.getAttribute("data-shot"));
    });
    listeners.forEach(function (fn) { fn(t); });
  }
  function choose(id) {
    var t = find(id) || SNAP;
    current = t;
    try { localStorage.setItem(KEY, t.id); } catch (e) {}
    apply(t);
    renderDock();
  }

  // Shared with site.js (the demo crops the themed library screenshot).
  window.SnapwrightTheme = {
    shot: function (name) { return "img/themes/" + current.id + "/" + name + ".png"; },
    onChange: function (fn) { listeners.push(fn); }
  };
  apply(current);

  // The theme dock: opened from the header's Theme button, ← → cycle, a chip per theme.
  var dock, chips, label;
  function renderDock() {
    if (!dock) return;
    label.textContent = current.name + " · " + (current.original ? "original" : current.mode);
    Array.prototype.forEach.call(chips.children, function (chip) {
      var on = chip.getAttribute("data-id") === current.id;
      chip.classList.toggle("on", on);
      if (on && !dock.hidden) chip.scrollIntoView({ block: "nearest", inline: "nearest" });
    });
  }
  function cycle(step) {
    var i = THEMES.indexOf(current);
    choose(THEMES[(i + step + THEMES.length) % THEMES.length].id);
  }
  function build() {
    var button = document.querySelector("[data-theme-toggle]");
    if (!button) return;
    dock = document.createElement("div");
    dock.className = "theme-dock";
    dock.hidden = true;
    dock.setAttribute("role", "dialog");
    dock.setAttribute("aria-label", "Theme");
    dock.innerHTML =
      '<div class="dock-head"><b>Theme</b><span class="dock-name"></span>' +
      '<span class="dock-keys"><kbd>←</kbd><kbd>→</kbd>Cycle</span>' +
      '<button type="button" class="dock-close" aria-label="Close">×</button></div>' +
      '<div class="dock-chips"></div>';
    label = dock.querySelector(".dock-name");
    chips = dock.querySelector(".dock-chips");
    THEMES.forEach(function (t) {
      var chip = document.createElement("button");
      chip.type = "button";
      chip.className = "chip";
      chip.title = t.name;
      chip.setAttribute("data-id", t.id);
      chip.innerHTML =
        '<span class="swatch" style="background:' + t.background + '"><i style="background:' + t.bright_foreground + '"></i>' +
        '<span><u style="background:' + t.accent + '"></u><u style="background:' + t.red + '"></u>' +
        '<u style="background:' + t.green + '"></u><u style="background:' + t.yellow + '"></u></span></span>' +
        '<span class="chip-name"></span>';
      chip.querySelector(".chip-name").textContent = t.name;
      chip.addEventListener("click", function () { choose(t.id); });
      chips.appendChild(chip);
    });
    document.body.appendChild(dock);
    button.addEventListener("click", function () {
      dock.hidden = !dock.hidden;
      button.setAttribute("aria-expanded", String(!dock.hidden));
    });
    dock.querySelector(".dock-close").addEventListener("click", function () {
      dock.hidden = true;
      button.setAttribute("aria-expanded", "false");
    });
    window.addEventListener("keydown", function (e) {
      if (dock.hidden || /input|textarea/i.test(e.target.tagName)) return;
      if (e.key === "ArrowRight") { e.preventDefault(); cycle(1); }
      else if (e.key === "ArrowLeft") { e.preventDefault(); cycle(-1); }
      else if (e.key === "Escape") { dock.hidden = true; }
    });
    renderDock();
    apply(current);
  }
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", build);
  else build();
})();
