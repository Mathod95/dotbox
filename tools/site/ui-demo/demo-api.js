// La démo de l'interface web pour la page d'accueil (D148): les vrais fichiers de web/ (index.html,
// app.css, app.js, sans changement), et ce script, chargé avant app.js, qui répond à la place de
// l'API du service (/api/v1) avec des données d'exemple. Rien ne sort de la page: fetch et
// EventSource sont remplacés. Domaine example.com, utilisateur admin.
(function () {
  "use strict";
  var DOMAIN = "example.com";
  var CATALOG = [
    ["autobrr", "Release grabber for trackers' announce channels", "v1.91.1", "two_factor"],
    ["code-server", "VS Code in the browser", "4.146.0", "two_factor"],
    ["homarr", "Dashboard for your apps", "v2.14.0", "two_factor"],
    ["jellyfin", "Media server", "12.4", "app"],
    ["maintainerr", "Rule-based library cleanup", "3.33.0", "two_factor"],
    ["plex", "Media server", "1.43.5.11079-b3b13e506", "app"],
    ["prowlarr", "Indexer manager for the *arr apps", "3.0.2.5780-ls169", "two_factor"],
    ["qbittorrent", "BitTorrent client", "5.3.0-2", "two_factor"],
    ["qui", "Web interface for qBittorrent", "v1.36.1", "two_factor"],
    ["radarr", "Movie collection manager", "7.0.3.10918-ls328", "two_factor"],
    ["seerr", "Media requests", "v3.5.2", "app"],
    ["sonarr", "TV show collection manager", "4.0.20.3014-ls329", "two_factor"],
    ["tautulli", "Plex statistics and history", "v2.19.1", "two_factor"],
    ["uptime-kuma", "Uptime monitoring", "2.5.12-slim-rootless", "two_factor"],
    ["vaultwarden", "Password manager (Bitwarden compatible)", "1.37.5", "app"],
    ["wg-easy", "WireGuard VPN with a web interface", "15.5.0", "two_factor"],
    ["wizarr", "Media server invitations", "v2026.10.1", "public"]
  ];
  function cat(name) { for (var i = 0; i < CATALOG.length; i++) if (CATALOG[i][0] === name) return CATALOG[i]; }

  // l'état du serveur d'exemple: Sonarr a une version de retard, pour la vue Updates
  var installed = {
    jellyfin: { state: "running", health: "healthy" },
    radarr: { state: "running", health: "healthy" },
    sonarr: { state: "running", health: "healthy", version: "4.0.19.2990-ls327" },
    prowlarr: { state: "running", health: "healthy" },
    qbittorrent: { state: "running", health: "healthy" },
    seerr: { state: "running", health: "healthy" }
  };
  var config = { domain: DOMAIN, user: "admin", ca: "letsencrypt", "backup.schedule": "daily 04:00", "backup.keep": "7", "motd.enabled": "true", "motd.style": "compact" };
  Object.keys(installed).forEach(function (n) { config["apps." + n + ".subdomain"] = n; config["apps." + n + ".access"] = cat(n)[3]; });
  var plan = [];
  var backup = { ok: true, started: "Nov 24, 04:00", archives: 7, bytes: 48 * 1024 * 1024, next: "Tue 2026-11-25 04:00" };

  function status() {
    return {
      core: [
        { name: "docker", version: "29.10.0", state: "running" },
        { name: "traefik", version: "v3.7.19", state: "running", health: "healthy" },
        { name: "authelia", version: "4.39.28", state: "running", health: "healthy" },
        { name: "crowdsec", version: "v1.8.2", state: "running", health: "healthy" }
      ],
      apps: Object.keys(installed).sort().map(function (n) {
        var a = installed[n], c = cat(n);
        return {
          name: n, url: "https://" + config["apps." + n + ".subdomain"] + "." + DOMAIN, state: a.state,
          health: a.state === "running" ? a.health : undefined, access: config["apps." + n + ".access"],
          setup_pending: false, version: a.version || c[2]
        };
      }),
      backup: backup
    };
  }
  var doctor = { date: "2026-11-24T05:00:00", checks: [
    { level: "ok", section: "Core", detail: "Traefik, Authelia and CrowdSec running" },
    { level: "warn", section: "SSH", detail: "ssh accepts passwords", hint: "add your key, then turn on the SSH hardening" }
  ] };

  // ---------- les tâches: une suite d'événements, envoyés un par un
  var jobs = {}, nextJob = 1;
  function steps(list, apply) { var id = "demo-" + nextJob++; jobs[id] = { list: list, apply: apply }; return id; }
  function events(id) {
    var j = jobs[id], out = [{ t: "start", total: j.list.length, first: 1 }];
    j.list.forEach(function (s) {
      out.push({ t: "step", name: s[0], doing: s[1] });
      if (s[2]) out.push({ t: "sub", verb: s[2][0], detail: s[2][1] });
      out.push({ t: "done", summary: s[3] });
    });
    out.push({ t: "end", code: 0 });
    return out;
  }
  function install(name) {
    var c = cat(name);
    return steps([
      ["Image", "the app's image", ["pull", "its image, version " + c[2]], c[2]],
      ["Folders", "/opt/" + name, ["create", "/opt/" + name + "/config"], "owned by admin"],
      ["Compose", "compose.yml", ["write", "/opt/" + name + "/compose.yml"], name + "." + DOMAIN + ", " + c[3]],
      ["Start", "the container", ["start", name], "running"],
      ["Ready", "its healthcheck", ["wait", name + " healthy"], "https://" + name + "." + DOMAIN]
    ], function () {
      installed[name] = { state: "running", health: "healthy" };
      config["apps." + name + ".subdomain"] = name; config["apps." + name + ".access"] = c[3];
    });
  }
  function simple(name, label, doing, summary, apply) { return steps([[label, doing, null, summary]], apply); }

  // ---------- les routes
  function json(v, code) { return Promise.resolve(new Response(JSON.stringify(v), { status: code || 200, headers: { "Content-Type": "application/json" } })); }
  function jobReply(id, what) { return json({ job: id, what: what }); }
  function route(method, path, body) {
    var m;
    if (method === "GET" && path === "/status") return json(status());
    if (path === "/doctor") return json(doctor);
    if (method === "GET" && path === "/diff") return json(plan);
    if (method === "GET" && path === "/catalog") return json(CATALOG.map(function (c) { return { name: c[0], role: c[1], version: c[2], installed: !!installed[c[0]] }; }));
    if (method === "GET" && path === "/config") return json(Object.keys(config).map(function (k) { return { key: k, value: config[k] }; }));
    if (method === "PUT" && (m = path.match(/^\/config\/(.+)$/))) {
      var key = decodeURIComponent(m[1]), old = config[key];
      if (key === "domain" || key === "user") return json({ error: key + " changes in the terminal" }, 400);
      config[key] = body.value;
      var app = key.match(/^apps\.([^.]+)\./);
      plan.push({ app: app ? app[1] : "dotbox", what: key + ": " + old + " → " + body.value, sync: app ? "rewrite its compose.yml and restart it" : "apply the new value" });
      return json({ changed: old !== body.value, old: old, new: body.value });
    }
    if (method === "POST" && path === "/apps") { var n = body.apps[0]; return jobReply(install(n), "install " + n); }
    if (method === "DELETE" && (m = path.match(/^\/apps\/([^/?]+)/))) {
      var r = decodeURIComponent(m[1]);
      return jobReply(simple(r, "Remove", r, "stopped, config kept", function () { delete installed[r]; delete config["apps." + r + ".subdomain"]; delete config["apps." + r + ".access"]; }), "remove " + r);
    }
    if (method === "POST" && (m = path.match(/^\/apps\/([^/]+)\/(start|stop|restart)$/))) {
      var a = decodeURIComponent(m[1]), act = m[2];
      return jobReply(simple(a, act.charAt(0).toUpperCase() + act.slice(1), a, act === "stop" ? "stopped" : "running",
        function () { installed[a].state = act === "stop" ? "exited" : "running"; }), act + " " + a);
    }
    if (method === "POST" && path === "/update") {
      var names = body.all ? Object.keys(installed) : body.apps;
      return jobReply(steps(names.map(function (x) { return [x, body.rollback ? "the previous image" : "the catalog's version", null, cat(x)[2]]; }),
        function () { names.forEach(function (x) { delete installed[x].version; }); }), (body.rollback ? "rollback " : "update ") + names.join(" "));
    }
    if (method === "POST" && path === "/backup") {
      var what = body.app ? [body.app] : ["dotbox"].concat(Object.keys(installed).sort());
      return jobReply(steps([["Key", "the lock of the archives (age)", null, "backup.key"]].concat(what.map(function (x) { return [x, "its archive", ["write", "/backups/" + x + "-2026-11-24-1200.tar.zst.age"], x === "dotbox" ? "11 kB" : "6 MB, healthy"]; })),
        function () { backup = { ok: true, started: "Nov 24, 12:00", archives: what.length, bytes: what.length * 6 * 1024 * 1024, next: backup.next }; }), "backup");
    }
    if (method === "POST" && path === "/sync") {
      return jobReply(steps(plan.map(function (p) { return [p.app, p.what, null, "done"]; }), function () { plan = []; }), "sync");
    }
    return json({ error: "not in this demo" }, 404);
  }

  var API = "/api/v1";
  var realFetch = window.fetch;
  window.fetch = function (url, opt) {
    var u = String(url), i = u.indexOf(API);
    if (i < 0) return realFetch.apply(this, arguments);
    var path = u.slice(i + API.length), method = (opt && opt.method) || "GET", body = opt && opt.body ? JSON.parse(opt.body) : undefined;
    // les logs d'une app: quelques lignes, puis le flux reste ouvert
    var lm = path.match(/^\/apps\/([^/]+)\/logs/);
    if (lm) {
      var app = decodeURIComponent(lm[1]), enc = new TextEncoder();
      var lines = ["[info] " + app + " starting", "[info] listening on :8080", "[info] " + app + "." + DOMAIN + " ready", "[info] health check ok"];
      var stream = new ReadableStream({ start: function (ctl) {
        lines.forEach(function (l, k) { setTimeout(function () { try { ctl.enqueue(enc.encode("2026-11-24 12:0" + k + " " + l + "\n")); } catch (e) {} }, 200 * (k + 1)); });
      } });
      return Promise.resolve(new Response(stream, { status: 200 }));
    }
    return route(method, path.replace(/\?.*$/, ""), body);
  };

  window.EventSource = function (url) {
    var self = this, id = String(url).match(/\/jobs\/([^/]+)\/events/)[1], list = events(id), k = 0, closed = false;
    function next() {
      if (closed || k >= list.length) return;
      var e = list[k++];
      if (e.t === "end" && jobs[id].apply) jobs[id].apply();
      if (self.onmessage) self.onmessage({ data: JSON.stringify(e) });
      setTimeout(next, e.t === "step" ? 450 : 120);
    }
    this.close = function () { closed = true; };
    setTimeout(next, 200);
  };
})();
