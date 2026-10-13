// Snapwright site: platform-aware download links and the "try it here" capture demo.
(function () {
  "use strict";

  var IMG_W = 1200, IMG_H = 760;
  // Regions of the library screenshot the demo captures on its own: [x0, y0, x1, y1] as
  // fractions (img/themes/<theme>/library.png, from scripts/site-shots.sh).
  var REGIONS = [
    [0.195, 0.13, 0.362, 0.37],
    [0.725, 0.035, 0.995, 0.29],
    [0.367, 0.385, 0.53, 0.625],
    [0.0, 0.0, 0.18, 0.31],
    [0.539, 0.385, 0.703, 0.625]
  ];
  // Stable names the release workflows upload next to the versioned files
  // (scripts/upload-release-assets.py), so these always fetch the newest build.
  var LATEST = "https://github.com/jeremielumandong/Snapwright/releases/latest/download/";
  var PLAT = {
    mac: { short: "macOS", label: "Download for macOS", sub: "Universal DMG · Apple silicon & Intel", mod: "⌘", alt: "Option", file: "Snapwright-macos-universal.dmg" },
    windows: { short: "Windows", label: "Download for Windows", sub: "Installer · Windows 10 & 11 · x64", mod: "Ctrl", alt: "Alt", file: "Snapwright-windows-x64-setup.exe" },
    linux: { short: "Linux", label: "Download for Linux", sub: "Installs the latest AppImage, checks its SHA-256, adds a launcher and the snapwright CLI. No sudo.", mod: "Ctrl", alt: "Alt", file: "Snapwright-linux-x86_64.AppImage" }
  };

  function $(sel, root) { return (root || document).querySelector(sel); }
  function $$(sel) { return Array.prototype.slice.call(document.querySelectorAll(sel)); }
  function setText(sel, text) { $$(sel).forEach(function (el) { el.textContent = text; }); }

  // Platform
  var ua = ((navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || navigator.userAgent || "").toLowerCase();
  var plat = /mac|iphone|ipad/.test(ua) ? "mac" : /win/.test(ua) ? "windows" : /linux|x11|cros/.test(ua) ? "linux" : "mac";
  var P = PLAT[plat];
  setText("[data-os-short]", P.short);
  setText("[data-dl-label]", P.label);
  setText("[data-dl-sub]", P.sub);
  setText("[data-mod]", P.mod);
  setText("[data-alt]", P.alt);
  $$("[data-dl-href]").forEach(function (el) { el.href = LATEST + P.file; });
  // Linux gets the one-line installer (install.sh) in place of a file, as on Switchyard's site.
  if (plat === "linux") {
    $$("[data-dl-href]").forEach(function (el) { el.hidden = true; });
    $$("[data-linux-cmd]").forEach(function (el) { el.hidden = false; });
  }
  $$("[data-copy]").forEach(function (btn) {
    btn.addEventListener("click", function () {
      var text = btn.parentNode.querySelector("code").textContent;
      var done = function () {
        btn.textContent = "Copied";
        setTimeout(function () { btn.textContent = "Copy"; }, 1600);
      };
      if (navigator.clipboard && navigator.clipboard.writeText) navigator.clipboard.writeText(text).then(done, function () {});
    });
  });
  $$("[data-platform]").forEach(function (el) {
    el.classList.toggle("mine", el.getAttribute("data-platform") === plat);
  });

  // Demo
  var stage = $("#stage");
  if (!stage) return;
  var hint = $(".demo-head .hint");
  var dim = $(".dim", stage), sel = $(".sel", stage), selLabel = $(".sel span", stage);
  var chH = $(".ch-h", stage), chV = $(".ch-v", stage), coord = $(".coord", stage);
  var flash = $(".flash", stage), preview = $(".preview", stage), previewCrop = $(".preview .crop", stage);
  var shots = $("#shots"), capCount = $("#cap-count");

  var state = { phase: "idle", cx: 0.5, cy: 0.45, sel: null };
  var captures = 0;
  var autoToken = null, userDrag = false, pvTimer = 0;

  function lerp(a, b, t) { return a + (b - a) * t; }
  function ease(t) { return t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2; }
  function pct(v) { return (v * 100).toFixed(3) + "%"; }
  function rect(s) {
    return { x: Math.min(s.x0, s.x1), y: Math.min(s.y0, s.y1), w: Math.abs(s.x1 - s.x0), h: Math.abs(s.y1 - s.y0) };
  }

  function render() {
    var s = state.sel;
    var active = state.phase === "armed" || state.phase === "drag";
    hint.classList.toggle("armed", active);
    dim.hidden = !(state.phase === "armed" && !s);
    var r = s ? rect(s) : null;
    sel.hidden = !(r && (r.w > 0.002 || r.h > 0.002));
    if (!sel.hidden) {
      sel.style.left = pct(r.x); sel.style.top = pct(r.y);
      sel.style.width = pct(r.w); sel.style.height = pct(r.h);
      sel.classList.toggle("low", r.y < 0.06);
      selLabel.textContent = Math.round(r.w * IMG_W) + " × " + Math.round(r.h * IMG_H);
    }
    chH.hidden = chV.hidden = coord.hidden = !active;
    if (active) {
      chH.style.top = coord.style.top = pct(state.cy);
      chV.style.left = coord.style.left = pct(state.cx);
      coord.textContent = Math.round(state.cx * IMG_W) + ", " + Math.round(state.cy * IMG_H);
    }
  }
  function set(patch) {
    for (var k in patch) state[k] = patch[k];
    render();
  }

  // Shows the region r of the library screenshot (in the current theme) in el, scaled to
  // el's height.
  function crop(el, r) {
    var theme = window.SnapwrightTheme;
    el.style.backgroundImage = "url(" + (theme ? theme.shot("library") : "img/themes/snapwright/library.png") + ")";
    el.style.aspectRatio = Math.round(r.w * IMG_W) + " / " + Math.max(1, Math.round(r.h * IMG_H));
    el.style.backgroundSize = (100 / r.w) + "% " + (100 / r.h) + "%";
    el.style.backgroundPosition = (r.w >= 0.999 ? 0 : (r.x / (1 - r.w)) * 100) + "% " + (r.h >= 0.999 ? 0 : (r.y / (1 - r.h)) * 100) + "%";
  }

  function capture() {
    if (!state.sel) return;
    var r = rect(state.sel);
    crop(previewCrop, r);
    preview.classList.add("on");

    var empty = $(".empty", shots);
    if (empty) empty.remove();
    var shot = document.createElement("div");
    shot.className = "shot";
    var thumb = document.createElement("div");
    thumb.className = "thumb";
    var c = document.createElement("div");
    c.className = "crop";
    crop(c, r);
    thumb.appendChild(c);
    var label = document.createElement("span");
    label.textContent = Math.round(r.w * IMG_W) + "×" + Math.round(r.h * IMG_H) + " · " +
      new Date().toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
    shot.appendChild(thumb);
    shot.appendChild(label);
    shots.insertBefore(shot, shots.firstChild);
    while (shots.children.length > 7) shots.removeChild(shots.lastChild);
    captures++;
    capCount.textContent = captures + (captures === 1 ? " capture" : " captures") + ", searchable";

    flash.classList.add("on");
    setTimeout(function () { flash.classList.remove("on"); }, 40);
    set({ phase: "idle", sel: null });
  }
  function hidePreview() { preview.classList.remove("on"); }

  // Autoplay: sweeps across REGIONS until the visitor takes over.
  function wait(ms, tok) {
    return new Promise(function (res, rej) {
      setTimeout(function () { autoToken === tok ? res() : rej("stop"); }, ms);
    });
  }
  function tween(ms, fn, tok) {
    return new Promise(function (res, rej) {
      var t0 = performance.now();
      function step(now) {
        if (autoToken !== tok) return rej("stop");
        var t = Math.min(1, (now - t0) / ms);
        fn(ease(t));
        if (t < 1) requestAnimationFrame(step); else res();
      }
      requestAnimationFrame(step);
    });
  }
  function runAuto() {
    var tok = autoToken = {};
    var i = 0;
    function loop() {
      var r = REGIONS[i++ % REGIONS.length];
      var fx = 0.5, fy = 0.45;
      set({ phase: "armed", sel: null, cx: fx, cy: fy });
      return wait(350, tok)
        .then(function () { return tween(650, function (t) { set({ cx: lerp(fx, r[0], t), cy: lerp(fy, r[1], t) }); }, tok); })
        .then(function () { return wait(120, tok); })
        .then(function () {
          return tween(850, function (t) {
            var x = lerp(r[0], r[2], t), y = lerp(r[1], r[3], t);
            set({ phase: "drag", cx: x, cy: y, sel: { x0: r[0], y0: r[1], x1: x, y1: y } });
          }, tok);
        })
        .then(function () { return wait(220, tok); })
        .then(function () { capture(); return wait(2300, tok); })
        .then(function () { hidePreview(); return wait(900, tok); })
        .then(loop);
    }
    wait(900, tok).then(loop).catch(function () {});
  }
  function stopAuto() { autoToken = null; }

  function pt(e) {
    var r = stage.getBoundingClientRect();
    return {
      x: Math.max(0, Math.min(1, (e.clientX - r.left) / r.width)),
      y: Math.max(0, Math.min(1, (e.clientY - r.top) / r.height))
    };
  }
  stage.addEventListener("pointerdown", function (e) {
    stopAuto();
    try { stage.setPointerCapture(e.pointerId); } catch (_) {}
    var p = pt(e);
    userDrag = true;
    hidePreview();
    set({ phase: "drag", cx: p.x, cy: p.y, sel: { x0: p.x, y0: p.y, x1: p.x, y1: p.y } });
  });
  stage.addEventListener("pointermove", function (e) {
    if (autoToken) return;
    var p = pt(e);
    if (state.phase === "drag" && userDrag) {
      state.sel.x1 = p.x; state.sel.y1 = p.y;
      set({ cx: p.x, cy: p.y });
    } else if (state.phase === "armed") {
      set({ cx: p.x, cy: p.y });
    }
  });
  function release() {
    if (!userDrag) return;
    userDrag = false;
    var s = state.sel;
    if (!s || Math.abs(s.x1 - s.x0) < 0.02 || Math.abs(s.y1 - s.y0) < 0.02) { set({ phase: "armed", sel: null }); return; }
    capture();
    clearTimeout(pvTimer);
    pvTimer = setTimeout(hidePreview, 3200);
  }
  stage.addEventListener("pointerup", release);
  stage.addEventListener("pointercancel", release);
  window.addEventListener("keydown", function (e) {
    if (e.code === "Digit2" && e.shiftKey && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      stopAuto();
      hidePreview();
      set({ phase: "armed", sel: null });
    } else if (e.key === "Escape" && state.phase !== "idle") {
      stopAuto();
      set({ phase: "idle", sel: null });
    }
  });

  var reduced = window.matchMedia && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  if (!reduced) runAuto();
})();
