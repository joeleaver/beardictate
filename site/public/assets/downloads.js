// Renders the download list from /downloads/releases.json, so publishing a build is a
// data change (see sites/beardictate/publish.sh), never a page edit. Everything from the
// manifest goes in via textContent; file names are checked before they become links.
(function () {
  var root = document.getElementById("platforms");
  if (!root) return;

  var SAFE_FILE = /^[A-Za-z0-9][A-Za-z0-9._+-]*$/;

  function el(tag, cls, text) {
    var e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }

  function cmpVersion(a, b) {
    var pa = String(a).split(/[.-]/), pb = String(b).split(/[.-]/);
    for (var i = 0; i < Math.max(pa.length, pb.length); i++) {
      var x = parseInt(pa[i] || "0", 10), y = parseInt(pb[i] || "0", 10);
      if (isNaN(x) || isNaN(y)) { if (pa[i] !== pb[i]) return (pa[i] || "") < (pb[i] || "") ? -1 : 1; continue; }
      if (x !== y) return x - y;
    }
    return 0;
  }

  function size(bytes) {
    if (!bytes) return "";
    if (bytes >= 1e9) return (bytes / 1e9).toFixed(2) + " GB";
    return Math.max(1, Math.round(bytes / 1e6)) + " MB";
  }

  function detectOs() {
    var p = ((navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || "") + " " + navigator.userAgent;
    if (/Win/i.test(p)) return "windows";
    if (/Mac|iPhone|iPad/i.test(p)) return "macos";
    if (/Linux|X11|CrOS/i.test(p) && !/Android/i.test(p)) return "linux";
    return "";
  }

  function copyButton(text) {
    var b = el("button", "copy", "Copy");
    b.type = "button";
    b.addEventListener("click", function () {
      var done = function () { b.textContent = "Copied"; setTimeout(function () { b.textContent = "Copy"; }, 1600); };
      if (navigator.clipboard && navigator.clipboard.writeText) {
        navigator.clipboard.writeText(text).then(done, function () { b.textContent = "Select and copy it"; });
      } else {
        b.textContent = "Select and copy it";
      }
    });
    return b;
  }

  function renderBuild(b) {
    var box = el("div", "build");
    var top = el("div", "build-top");
    var ok = !b.placeholder && SAFE_FILE.test(b.file || "") && /^[0-9a-f]{64}$/.test(b.sha256 || "");
    var a = el("a", "btn btn-primary", "Download " + (b.kind || "build"));
    if (ok) {
      a.href = "/downloads/" + encodeURIComponent(b.file);
      a.setAttribute("download", b.file);
    } else {
      a.setAttribute("aria-disabled", "true");
      a.setAttribute("role", "link");
    }
    top.appendChild(a);
    var meta = [ "Version " + b.version, size(b.size), b.date ].filter(Boolean).join(", ");
    top.appendChild(el("span", "build-meta", meta));
    if (b.placeholder) top.appendChild(el("span", "placeholder-tag", "Placeholder: not published yet"));
    box.appendChild(top);

    var sum = el("div", "sum");
    sum.appendChild(el("span", "label", "SHA-256"));
    if (ok) {
      sum.appendChild(el("code", null, b.sha256));
      sum.appendChild(copyButton(b.sha256));
    } else {
      sum.appendChild(el("code", null, "Appears here when the build is published"));
    }
    box.appendChild(sum);
    if (ok) {
      var f = el("div", "build-meta", b.file);
      f.style.marginTop = "0.35rem";
      box.appendChild(f);
    }
    return box;
  }

  function renderPlatform(p, builds, yours) {
    var li = el("li", "platform" + (yours ? " yours" : ""));
    li.id = p.id;
    var head = el("div");
    head.appendChild(el("h2", null, p.name));
    if (p.requirements) head.appendChild(el("p", "req", p.requirements));
    li.appendChild(head);

    var body = el("div");
    var real = builds.filter(function (b) { return !b.placeholder; });
    var shown = real.length ? real : builds;
    if (!real.length && p.pending) body.appendChild(el("p", "status", p.pending));

    var versions = [];
    shown.forEach(function (b) { if (versions.indexOf(b.version) < 0) versions.push(b.version); });
    versions.sort(function (a, b) { return cmpVersion(b, a); });

    if (versions.length) {
      shown.filter(function (b) { return b.version === versions[0]; }).forEach(function (b) { body.appendChild(renderBuild(b)); });
    }
    if (versions.length > 1) {
      var d = el("details", "older");
      d.appendChild(el("summary", null, "Earlier versions"));
      versions.slice(1).forEach(function (v) {
        shown.filter(function (b) { return b.version === v; }).forEach(function (b) { d.appendChild(renderBuild(b)); });
      });
      body.appendChild(d);
    }
    li.appendChild(body);
    return li;
  }

  fetch("/downloads/releases.json", { cache: "no-cache" })
    .then(function (r) { if (!r.ok) throw new Error("HTTP " + r.status); return r.json(); })
    .then(function (m) {
      var os = detectOs();
      var platforms = (m.platforms || []).slice();
      platforms.sort(function (a, b) { return (b.id === os) - (a.id === os); });
      root.textContent = "";
      platforms.forEach(function (p) {
        var builds = (m.builds || []).filter(function (b) { return b.platform === p.id; });
        root.appendChild(renderPlatform(p, builds, p.id === os));
      });
    })
    .catch(function () {
      root.textContent = "";
      root.appendChild(el("li", "load-error", "The download list didn’t load. Reload the page to try again."));
    });
})();
