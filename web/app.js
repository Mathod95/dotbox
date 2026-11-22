// L'interface web de Dot Box (Journal, 11.19 à 11.26, D102): du JavaScript simple, sans framework,
// qui appelle la même API que la CLI (/api/v1). Rien n'est gardé dans le navigateur (D105).
(function () {
  "use strict";

  var API = "/api/v1";
  var VIEWS = ["overview", "apps", "catalog", "updates", "backup", "settings"];
  var TITLES = { overview: "Overview", apps: "Apps", catalog: "Catalog", updates: "Updates", backup: "Backup", settings: "Settings" };
  var ACCESS = ["two_factor", "one_factor", "app", "public"];
  var CAS = ["letsencrypt", "letsencrypt-staging"];
  var busy = false;      // une tâche à la fois, comme le service
  var logsAbort = null;  // le flux des logs ouvert

  // ---------- petits outils
  function $(id) { return document.getElementById(id); }
  function el(tag, cls, text) {
    var e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text !== undefined && text !== null) e.textContent = text;
    return e;
  }
  function add(parent) { for (var i = 1; i < arguments.length; i++) if (arguments[i]) parent.appendChild(arguments[i]); return parent; }
  function btn(label, cls, fn) {
    var b = el("button", "btn" + (cls ? " " + cls : ""), label);
    b.type = "button";
    b.addEventListener("click", fn);
    return b;
  }
  function line(label, text, cls) {
    var l = el("div", "line");
    add(l, el("b", "", label), el("span", "grow" + (cls ? " " + cls : ""), text));
    return l;
  }
  function size(bytes) {
    var u = ["B", "kB", "MB", "GB", "TB"], i = 0, n = bytes;
    while (n >= 1000 && i < u.length - 1) { n /= 1000; i++; }
    return (i ? n.toFixed(n < 10 ? 1 : 0) : n) + " " + u[i];
  }
  function when(iso) {
    var d = new Date(iso);
    if (isNaN(d)) return iso;
    return d.toLocaleString(undefined, { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" });
  }
  function plural(n, word) { return n + " " + word + (n === 1 ? "" : "s"); }

  // ---------- l'API. Une session Authelia expirée renvoie vers auth.<domaine>: on le dit.
  function expired() {
    if (document.querySelector(".expired")) return;
    var box = el("div", "expired"), d = el("div");
    add(d, el("b", "", "Your session has ended"),
      el("span", "", "Authelia asks you to sign in again. Reload the page to go through it."),
      btn("Reload", "go", function () { location.reload(); }));
    document.body.appendChild(add(box, d));
  }
  function call(method, path, body) {
    var opt = { method: method, redirect: "manual", credentials: "same-origin", headers: {} };
    if (body !== undefined) { opt.headers["Content-Type"] = "application/json"; opt.body = JSON.stringify(body); }
    return fetch(API + path, opt).then(function (r) {
      if (r.type === "opaqueredirect" || r.status === 401) { expired(); throw new Error("session ended"); }
      return r.text().then(function (t) {
        var v = null;
        try { v = t ? JSON.parse(t) : null; } catch (e) { v = null; }
        if (!r.ok) throw new Error((v && v.error) || t || ("HTTP " + r.status));
        return v;
      });
    });
  }
  var get = function (p) { return call("GET", p); };

  // ---------- l'état d'un container, en une étiquette
  function stateOf(x) {
    if (x.state === "running") {
      if (x.health === "unhealthy") return ["unhealthy", "bad"];
      if (x.health === "starting") return ["starting", "warn"];
      return [x.health || "running", "ok"];
    }
    if (!x.state || x.state === "missing") return ["missing", "bad"];
    return [x.state === "exited" ? "stopped" : x.state, ""];
  }
  function stateTag(x) { var s = stateOf(x); return el("span", "st " + s[1], s[0]); }

  // ---------- une tâche: POST, puis ses événements en direct (GET /jobs/{id}/events)
  function job(method, path, body, after) {
    if (busy) { alert("Another job is running: wait for it to end."); return; }
    closeLogs();
    busy = true;
    setBusy();
    var panel = $("job"), steps = $("jobSteps"), end = $("jobEnd");
    steps.textContent = ""; end.textContent = "Starting…"; end.className = "end";
    $("jobWhat").textContent = "";
    panel.hidden = false;
    call(method, path, body).then(function (r) {
      $("jobWhat").textContent = r.what;
      follow(r.job, after);
    }).catch(function (e) {
      busy = false; setBusy();
      end.textContent = e.message; end.className = "end bad";
    });
  }

  function follow(id, after) {
    var steps = $("jobSteps"), end = $("jobEnd");
    var total = 0, n = 0, cur = null, finished = false;
    end.textContent = "";
    function tag(i) { return "[" + i + "/" + (total || "?") + "] "; }
    function scroll() { steps.scrollTop = steps.scrollHeight; }
    var src = new EventSource(API + "/jobs/" + id + "/events");
    src.onmessage = function (m) {
      var e;
      try { e = JSON.parse(m.data); } catch (x) { add(steps, el("li", "text", m.data)); scroll(); return; }
      switch (e.t) {
        case "start": total = e.total; n = e.first - 1; break;
        case "more": total += e.n; break;
        case "step":
          n++;
          cur = el("li", "run");
          cur.dataset.name = e.name;
          add(cur, el("span", "", "◇ " + tag(n) + e.name + "  " + e.doing), el("span", "sub"));
          add(steps, cur);
          break;
        case "sub": if (cur) cur.querySelector(".sub").textContent = e.verb + " " + e.detail; break;
        case "progress":
          if (cur) {
            var b = cur.querySelector(".bar i");
            if (!b) { var bar = el("span", "bar"); b = el("i"); add(cur, add(bar, b)); }
            b.style.width = Math.max(0, Math.min(100, e.pct)) + "%";
          }
          break;
        case "done":
          if (cur) { cur.className = "ok"; cur.textContent = "✓ " + tag(n) + cur.dataset.name + "  " + e.summary; cur = null; }
          break;
        case "cancel":
          if (cur) { cur.className = "cancel"; cur.textContent = "- " + tag(n) + cur.dataset.name + "  " + e.text; cur = null; }
          break;
        case "fail":
          var li = cur || add(steps, el("li"));
          li.className = "bad";
          li.textContent = "✗ " + tag(n) + (cur ? cur.dataset.name + "  " : "") + e.error;
          if (e.tail && e.tail.length) add(li, el("pre", "", e.tail.join("\n") + (e.log ? "\n\nlog " + e.log : "")));
          cur = null;
          break;
        case "error": add(steps, el("li", "bad", "error " + e.text)); break;
        case "result": break;
        case "end":
          finished = true;
          src.close();
          end.textContent = e.code === 0 ? "Done." : "Ended with an error (code " + e.code + ").";
          end.className = "end " + (e.code === 0 ? "ok" : "bad");
          busy = false; setBusy();
          if (after) after(e.code);
          render();
          break;
        default: add(steps, el("li", "text", m.data));
      }
      scroll();
    };
    src.onerror = function () {
      if (finished) return;
      src.close();
      busy = false; setBusy();
      end.textContent = "Lost the job's events. It may still be running: reload in a moment.";
      end.className = "end bad";
    };
  }

  function setBusy() {
    document.querySelectorAll("[data-job]").forEach(function (b) { b.disabled = busy; });
  }
  function jobBtn(label, cls, fn) { var b = btn(label, cls, fn); b.dataset.job = "1"; b.disabled = busy; return b; }

  // ---------- les logs d'une app, en flux, jusqu'à la fermeture du panneau
  function closeLogs() {
    if (logsAbort) { logsAbort.abort(); logsAbort = null; }
    $("logs").hidden = true;
  }
  function openLogs(app) {
    closeLogs();
    $("job").hidden = true;
    var pre = $("logsText");
    pre.textContent = "";
    $("logsWhat").textContent = app + ", live";
    $("logs").hidden = false;
    var ctl = new AbortController();
    logsAbort = ctl;
    fetch(API + "/apps/" + encodeURIComponent(app) + "/logs?follow=true&tail=200", { signal: ctl.signal, redirect: "manual", credentials: "same-origin" })
      .then(function (r) {
        if (r.type === "opaqueredirect") { expired(); return; }
        if (!r.ok) return r.json().then(function (v) { pre.textContent = (v && v.error) || ("HTTP " + r.status); });
        var reader = r.body.getReader(), dec = new TextDecoder();
        function pump() {
          return reader.read().then(function (c) {
            if (c.done) { pre.textContent += "\n(the log stream ended)"; return; }
            var atEnd = pre.scrollTop + pre.clientHeight >= pre.scrollHeight - 20;
            pre.textContent += dec.decode(c.value, { stream: true });
            // pas plus de 400 000 caractères à l'écran
            if (pre.textContent.length > 400000) pre.textContent = pre.textContent.slice(-300000);
            if (atEnd) pre.scrollTop = pre.scrollHeight;
            return pump();
          });
        }
        return pump();
      })
      .catch(function (e) { if (e.name !== "AbortError") pre.textContent += "\n" + e.message; });
  }

  // ---------- les vues
  var VIEW = {};

  VIEW.overview = function (m) {
    return Promise.all([
      get("/status"),
      get("/doctor").catch(function () { return null; }),
      get("/diff").catch(function (e) { return { error: e.message }; })
    ]).then(function (r) {
      var st = r[0], doc = r[1], diff = r[2];
      add(m, el("h2", "", "Core"));
      var cards = el("div", "cards");
      st.core.forEach(function (c) {
        add(cards, add(el("div", "card"), el("span", "n", c.name), el("span", "v", c.version || "not running"), stateTag(c)));
      });
      add(m, cards);
      add(m, el("h2", "", plural(st.apps.length, "app")));
      if (!st.apps.length) add(m, el("p", "note", "No app yet: install one from the Catalog."));
      var apps = el("div", "cards");
      st.apps.forEach(function (a) {
        var k = el("div", "card");
        var link = el("a", "v", a.url.replace(/^https:\/\//, ""));
        link.href = a.url; link.target = "_blank"; link.rel = "noopener noreferrer";
        add(k, el("span", "n", a.name), link, stateTag(a), el("span", "v", a.access + (a.setup_pending ? ", set up first" : "")));
        add(apps, k);
      });
      add(m, apps);

      // doctor
      var d = el("div", "line");
      add(d, el("b", "", "Doctor"));
      var nbad = doc ? doc.checks.filter(function (c) { return c.level !== "ok"; }).length : 0;
      var text = doc ? when(doc.date) + ": " + (nbad ? plural(nbad, "thing") + " to look at" : "all good") : "never run";
      var grow = add(d, el("span", "grow", text));
      add(d, btn("Run doctor", "", function (ev) {
        var b = ev.target; b.disabled = true; b.textContent = "Running…";
        call("POST", "/doctor").then(function () { render(); }).catch(function (e) { b.disabled = false; b.textContent = "Run doctor"; grow.textContent = e.message; grow.className = "grow err"; });
      }));
      if (doc) {
        var bad = doc.checks.filter(function (c) { return c.level !== "ok"; });
        if (bad.length) {
          var ul = el("ul", "checks");
          bad.forEach(function (c) {
            var li = el("li");
            var head = el("span", "chk " + (c.level === "error" ? "bad" : "warn"));
            if (c.section) add(head, el("i", "", c.section));
            add(head, document.createTextNode(c.detail));
            add(li, head);
            if (c.hint) add(li, el("span", "h", c.hint));
            add(ul, li);
          });
          add(d, ul);
        }
      }
      add(m, d);

      // la sauvegarde
      add(m, backupLine(st.backup));

      // l'écart avec dotbox.yml
      var g = el("div", "line");
      add(g, el("b", "", "dotbox.yml"));
      if (diff.error) add(g, el("span", "grow err", diff.error));
      else {
        var gaps = diff.filter(function (x) { return !x.info; });
        add(g, el("span", "grow", gaps.length ? plural(gaps.length, "difference") + " with what runs" : "0 differences, all as wanted"));
        if (gaps.length) { var a = el("a", "", "See the plan"); a.href = "#settings"; add(g, a); }
      }
      add(m, g);
    });
  };

  function backupLine(b) {
    if (!b) return line("Backup", "no backup yet");
    var t = (b.ok ? "" : "failed: " + b.error + ". ") + b.started + ", " + plural(b.archives, "archive") + ", " + size(b.bytes) + (b.next ? ". Next: " + b.next : ". No schedule (backup.schedule: off)");
    return line("Backup", t, b.ok ? "" : "err");
  }

  VIEW.apps = function (m) {
    return get("/status").then(function (st) {
      add(m, el("h2", "", "Installed apps"));
      if (!st.apps.length) { add(m, el("p", "note", "No app yet: install one from the Catalog.")); return; }
      var rows = el("div", "rows apps");
      st.apps.forEach(function (a) {
        var r = el("div", "row");
        var link = el("a", "", a.url.replace(/^https:\/\//, ""));
        link.href = a.url; link.target = "_blank"; link.rel = "noopener noreferrer";
        add(r, el("span", "n", a.name), add(el("span"), link), add(el("span"), stateTag(a)), el("span", "", a.access));
        var acts = el("span", "acts");
        var name = encodeURIComponent(a.name);
        if (a.state !== "running") acts.appendChild(jobBtn("Start", "go", function () { job("POST", "/apps/" + name + "/start"); }));
        else {
          acts.appendChild(jobBtn("Stop", "", function () { job("POST", "/apps/" + name + "/stop"); }));
          acts.appendChild(jobBtn("Restart", "", function () { job("POST", "/apps/" + name + "/restart"); }));
        }
        acts.appendChild(btn("Logs", "", function () { openLogs(a.name); }));
        acts.appendChild(jobBtn("Remove", "danger", function () { removeApp(a.name); }));
        add(r, acts);
        add(rows, r);
      });
      add(m, rows);
      add(m, el("p", "note", "Remove keeps /opt/<app>/config: installing again picks it up. Purge erases it too."));
    });
  };

  function removeApp(app) {
    if (!confirm("Remove " + app + "?\n\nIts container stops and its compose.yml goes. /opt/" + app + "/config is kept: installing it again picks it up.")) return;
    var purge = confirm("Also erase /opt/" + app + " and its image (purge)?\n\nOK: erase everything, it cannot be undone.\nCancel: keep the configuration.");
    if (purge && !confirm("Erase /opt/" + app + " for good?")) return;
    job("DELETE", "/apps/" + encodeURIComponent(app) + (purge ? "?purge=true" : ""));
  }

  VIEW.catalog = function (m) {
    return get("/catalog").then(function (list) {
      var free = list.filter(function (x) { return !x.installed; });
      var head = el("div", "head");
      var search = el("input", "search");
      search.type = "search"; search.placeholder = "Search the catalog"; search.setAttribute("aria-label", "Search the catalog");
      add(head, el("h2", "", "Catalog, " + plural(free.length, "app") + " not installed"), search);
      add(m, head);
      var cards = el("div", "cards");
      function draw() {
        var q = search.value.trim().toLowerCase();
        cards.textContent = "";
        free.filter(function (x) { return !q || x.name.indexOf(q) >= 0 || x.role.toLowerCase().indexOf(q) >= 0; }).forEach(function (x) {
          var k = el("div", "card");
          add(k, el("span", "n", x.name), el("span", "v", x.role), el("span", "v", x.version));
          add(k, add(el("div", "acts"), jobBtn("Install", "go", function () { job("POST", "/apps", { apps: [x.name] }); })));
          add(cards, k);
        });
        if (!cards.children.length) add(cards, el("p", "note", free.length ? "Nothing matches." : "Every app of the catalog is installed."));
      }
      search.addEventListener("input", draw);
      draw();
      add(m, cards);
      add(m, el("p", "note", "An app outside the catalog stays in the terminal, as root: sudo dotbox add <app>."));
    });
  };

  VIEW.updates = function (m) {
    return Promise.all([get("/status"), get("/catalog")]).then(function (r) {
      var latest = {};
      r[1].forEach(function (x) { latest[x.name] = x.version; });
      var apps = r[0].apps.map(function (a) { return { name: a.name, current: a.version, latest: latest[a.name] || "" }; });
      var behind = apps.filter(function (a) { return a.latest && a.current && a.current !== a.latest; });
      var head = el("div", "head");
      add(head, el("h2", "", plural(behind.length, "app") + " behind the catalog"));
      if (behind.length) add(head, jobBtn("Update all", "go", function () { job("POST", "/update", { all: true }); }));
      add(m, head);
      var rows = el("div", "rows upd");
      apps.forEach(function (a) {
        var row = el("div", "row");
        var up = !a.latest || !a.current || a.current === a.latest;
        add(row, el("span", "n", a.name), el("span", "", a.current || "not running"), el("span", up ? "" : "st warn", up ? "up to date" : "→ " + a.latest), el("span"));
        var acts = el("span", "acts");
        if (!up) acts.appendChild(jobBtn("Update", "go", function () { job("POST", "/update", { apps: [a.name] }); }));
        acts.appendChild(jobBtn("Roll back", "", function () {
          if (confirm("Go back to the image " + a.name + " used before its last update?")) job("POST", "/update", { apps: [a.name], rollback: true });
        }));
        add(row, acts);
        add(rows, row);
      });
      add(m, rows);
      add(m, el("p", "note", "Versions are compared with the catalog kept on the server. An update reads the catalog on GitHub again first, one app at a time, the next one only once the previous one runs."));
    });
  };

  VIEW.backup = function (m) {
    return get("/status").then(function (st) {
      add(m, el("h2", "", "Backup"));
      add(m, backupLine(st.backup));
      var row = el("div", "line");
      var pick = el("select");
      pick.setAttribute("aria-label", "What to back up");
      add(pick, el("option", "", "Everything"), el("option", "", "dotbox"));
      pick.options[0].value = ""; pick.options[1].value = "dotbox";
      st.apps.forEach(function (a) { var o = el("option", "", a.name); o.value = a.name; add(pick, o); });
      add(row, el("b", "", "Now"), add(el("span", "grow"), pick), jobBtn("Back up", "go", function () {
        job("POST", "/backup", pick.value ? { app: pick.value } : {});
      }));
      add(m, row);
      add(m, el("p", "note", "Each app stops for a short moment, then the next one starts only once it runs again. The archives go where backup.destinations says (local /backups by default), compressed and encrypted."));
      var p = el("p", "note");
      add(p, document.createTextNode("Restoring stays in the terminal, as root: "), el("code", "", "sudo dotbox restore latest --app <app>"));
      add(m, p);
    });
  };

  VIEW.settings = function (m) {
    return Promise.all([get("/config"), get("/diff").catch(function (e) { return { error: e.message }; })]).then(function (r) {
      var cfg = {};
      r[0].forEach(function (e) { cfg[e.key] = e.value; });

      add(m, el("h2", "", "dotbox.yml"));
      var gen = el("div", "rows");
      add(gen, cfgRow("domain", cfg.domain, null, "Changing the domain is a move: dotbox config set domain in the terminal, then dotbox sync."));
      add(gen, cfgRow("user", cfg.user, null, "The apps' files belong to it: it does not change."));
      add(gen, cfgRow("ca", cfg.ca, CAS));
      add(gen, cfgRow("backup.schedule", cfg["backup.schedule"], "text", "off, hourly, daily 04:00, or a day: sunday 05:00"));
      add(gen, cfgRow("backup.keep", cfg["backup.keep"], "text", "archives kept per app and per destination"));
      if ("motd.enabled" in cfg) {
        add(gen, cfgRow("motd.enabled", cfg["motd.enabled"], ["true", "false"], "the SSH welcome message, false puts Debian's back"));
        add(gen, cfgRow("motd.style", cfg["motd.style"], ["compact", "fastfetch", "banner", "ascii"], "from the next SSH login"));
      }
      add(m, gen);

      var names = Object.keys(cfg).filter(function (k) { return /^apps\.[^.]+\.subdomain$/.test(k); }).map(function (k) { return k.split(".")[1]; });
      if (names.length) {
        add(m, el("h2", "", "Apps: subdomain and access"));
        var apps = el("div", "rows");
        names.forEach(function (n) { add(apps, appRow(n, cfg)); });
        add(m, apps);
      }
      add(m, planBlock(r[1]));
    });
  };

  // une clé: lue seulement (kind null), un choix (liste) ou un texte
  function field(key, value, kind, hint) {
    var f = el("div", "field"), input;
    if (Array.isArray(kind)) {
      input = el("select");
      kind.forEach(function (v) { var o = el("option", "", v); o.value = v; add(input, o); });
      if (kind.indexOf(value) < 0) { var o = el("option", "", value); o.value = value; add(input, o); }
    } else { input = el("input"); input.type = "text"; input.spellcheck = false; }
    input.value = value === undefined ? "" : value;
    input.setAttribute("aria-label", key);
    if (kind === null) input.disabled = true;
    var msg = el("span", "note", hint || "");
    var saved = input.value;
    input.addEventListener("change", function () {
      if (input.value === saved) return;
      input.disabled = true;
      msg.className = "note"; msg.textContent = "checking…";
      call("PUT", "/config/" + encodeURIComponent(key), { value: input.value }).then(function (c) {
        saved = input.value;
        input.disabled = false;
        msg.className = c.warning ? "warn" : "note";
        msg.textContent = c.warning ? c.warning : (c.changed ? "written: " + c.old + " → " + c.new + ", see the plan below" : "same value");
        refreshPlan();
      }).catch(function (e) {
        input.value = saved;
        input.disabled = false;
        msg.className = "err"; msg.textContent = e.message;
      });
    });
    add(f, input, msg);
    return f;
  }
  function cfgRow(key, value, kind, hint) {
    var r = el("div", "row cfg");
    add(r, el("span", "n", key), field(key, value, kind, hint), el("span"));
    return r;
  }
  function appRow(app, cfg) {
    var r = el("div", "row cfg");
    add(r, el("span", "n", app),
      field("apps." + app + ".subdomain", cfg["apps." + app + ".subdomain"], "text"),
      field("apps." + app + ".access", cfg["apps." + app + ".access"], ACCESS));
    return r;
  }

  // le plan de dotbox sync: celui-là même est renvoyé à POST /sync (D93)
  var planHolder = null;
  function planBlock(diff) {
    var box = el("div");
    box.className = "plan-box";
    planHolder = box;
    drawPlan(box, diff);
    return box;
  }
  function refreshPlan() {
    if (!planHolder) return;
    get("/diff").then(function (d) { drawPlan(planHolder, d); }).catch(function (e) { drawPlan(planHolder, { error: e.message }); });
  }
  function drawPlan(box, diff) {
    box.textContent = "";
    var head = el("div", "head");
    add(box, head);
    if (diff.error) { add(head, el("h2", "", "The plan")); add(box, el("p", "note bad", diff.error)); return; }
    var gaps = diff.filter(function (x) { return !x.info; });
    var doable = gaps.filter(function (x) { return x.sync; });
    add(head, el("h2", "", "The plan, " + plural(gaps.length, "difference")));
    if (!diff.length) { add(box, el("p", "note", "Everything runs as dotbox.yml says.")); return; }
    var ul = el("ul", "plan");
    diff.forEach(function (x) {
      var cls = x.info ? "info" : (x.sync ? "" : "keep");
      var text = (x.info ? "= " : x.sync ? "~ " : "· ") + x.app + "  " + x.what + (x.sync ? "   → " + x.sync : "   (" + x.hint + ")");
      add(ul, el("li", cls, text));
    });
    add(box, ul);
    if (doable.length) {
      var row = el("div", "line");
      add(row, el("span", "grow", "dotbox sync applies the lines marked ~, nothing else. A version stays for Updates."));
      add(row, jobBtn("Apply this plan", "go", function () {
        if (confirm("Apply these " + plural(doable.length, "change") + "?")) job("POST", "/sync", { plan: diff });
      }));
      add(box, row);
    }
  }

  // ---------- la navigation: #overview, #apps... (rien de gardé ailleurs)
  function current() {
    var v = location.hash.replace(/^#/, "");
    return VIEWS.indexOf(v) >= 0 ? v : "overview";
  }
  function nav() {
    var n = $("nav"), cur = current();
    n.textContent = "";
    VIEWS.forEach(function (v) {
      var a = el("a", "", TITLES[v]);
      a.href = "#" + v;
      if (v === cur) a.setAttribute("aria-current", "page");
      add(n, a);
    });
  }
  var drawing = 0;
  function render() {
    var v = current(), m = $("main"), mine = ++drawing;
    nav();
    document.title = TITLES[v] + " · Dot Box";
    var fresh = el("div");
    fresh.className = "view";
    var wait = el("p", "loading", "Loading");
    m.textContent = "";
    add(m, wait);
    planHolder = null;
    VIEW[v](fresh).then(function () {
      if (mine !== drawing) return;
      m.textContent = "";
      while (fresh.firstChild) m.appendChild(fresh.firstChild);
      setBusy();
    }).catch(function (e) {
      if (mine !== drawing) return;
      m.textContent = "";
      add(m, el("p", "note bad", e.message));
    });
  }

  document.addEventListener("DOMContentLoaded", function () {
    var b = document.body;
    $("who").textContent = b.dataset.user || "?";
    $("ver").textContent = "v" + (b.dataset.version || "?");
    $("jobClose").addEventListener("click", function () { if (!busy) $("job").hidden = true; });
    $("logsClose").addEventListener("click", closeLogs);
    window.addEventListener("hashchange", render);
    render();
  });
})();
