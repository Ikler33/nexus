// sync.jsx — Sync (git) panel: changes, commit (+ secret-scan), remote, status.
(function () {
  const { useState } = React;
  const Icon = window.Icon;

  const STR = {
    ru: {
      title: "Синхронизация", sub: "Git · ветка main",
      changes: "Изменения", noChanges: "Изменений нет",
      commitPh: "Сообщение коммита…",
      committed: (m) => "Закоммичено: " + m,
      secretsHead: "Найдены секреты — коммит заблокирован",
      remote: "Удалённый репозиторий", remoteUrlPh: "https://github.com/user/notes.git",
      token: "Токен доступа", tokenPh: "ghp_••••••••••••", tokenSaved: "токен в keychain", tokenNone: "нет токена",
      connect: "Подключить",
      statusUpToDate: "Актуально", statusSynced: "Синхронизировано",
      statusConflict: "Конфликт — разрешите вручную", resolveConflicts: "Разрешить конфликты",
      sync: "Синхр.", commit: "Закоммитить", committing: "Коммичу…", syncing: "Синхронизирую…",
      A: "новый", M: "изменён", D: "удалён", R: "переименован",
    },
    en: {
      title: "Sync", sub: "Git · branch main",
      changes: "Changes", noChanges: "No changes",
      commitPh: "Commit message…",
      committed: (m) => "Committed: " + m,
      secretsHead: "Secrets found — commit blocked",
      remote: "Remote", remoteUrlPh: "https://github.com/user/notes.git",
      token: "Access token", tokenPh: "ghp_••••••••••••", tokenSaved: "token in keychain", tokenNone: "no token",
      connect: "Connect",
      statusUpToDate: "Up to date", statusSynced: "Synced",
      statusConflict: "Conflict — resolve manually", resolveConflicts: "Resolve conflicts",
      sync: "Sync", commit: "Commit", committing: "Committing…", syncing: "Syncing…",
      A: "added", M: "modified", D: "deleted", R: "renamed",
    },
  };

  const CHANGES = [
    { s: "M", dir: "Research/", name: "RAG Pipeline.md" },
    { s: "M", dir: "Projects/", name: "Nexus.md" },
    { s: "A", dir: "Daily/", name: "2026-06-09.md" },
    { s: "A", dir: "Research/", name: "KV-cache.md" },
    { s: "R", dir: "Research/", name: "Embeddings.md" },
    { s: "D", dir: "Inbox/", name: "scratch.md" },
  ];
  // secrets the scanner would flag in the staged diff
  const SECRETS = [
    { loc: "Projects/Nexus.md:42", kind: "API key" },
    { loc: "Daily/2026-06-09.md:8", kind: "token" },
  ];

  function SyncPanel({ lang, onClose, onResolveConflict, hasConflict, toast }) {
    const t = STR[lang] || STR.en;
    const [msg, setMsg] = useState("");
    const [scanSecrets, setScanSecrets] = useState(true); // demo: staged diff contains secrets
    const [result, setResult] = useState(null); // null | {ok, msg} | {blocked}
    const [remoteUrl, setRemoteUrl] = useState("");
    const [token, setToken] = useState("");
    const [tokenSaved, setTokenSaved] = useState(false);
    const [connected, setConnected] = useState(false);
    const [busy, setBusy] = useState(false);
    const [conflict, setConflict] = useState(hasConflict);

    function commit() {
      if (!msg.trim() || busy) return;
      setBusy(true); setResult(null);
      setTimeout(() => {
        setBusy(false);
        if (scanSecrets) { setResult({ blocked: true }); }
        else { setResult({ ok: true, msg: msg.trim() }); setMsg(""); }
      }, 700);
    }
    function doSync() {
      if (!connected || busy) return;
      setBusy(true);
      setTimeout(() => { setBusy(false); setConflict(false); toast && toast(t.statusSynced); }, 900);
    }
    function connect() {
      if (!remoteUrl.trim()) return;
      setConnected(true);
      if (token.trim()) setTokenSaved(true);
      toast && toast(lang === "ru" ? "Remote подключён" : "Remote connected");
    }

    const status = conflict ? "conflict" : (connected ? "synced" : null);

    return React.createElement("div", { className: "sync-scrim", onMouseDown: (e) => { if (e.target === e.currentTarget) onClose(); } },
      React.createElement("div", { className: "sync-panel", role: "dialog", "aria-label": t.title },
        React.createElement("div", { className: "sync-head" },
          React.createElement("div", { className: "sh-ic" }, React.createElement(Icon, { name: "git-branch", size: 18 })),
          React.createElement("div", { className: "sh-tt" },
            React.createElement("div", { className: "sh-title" }, t.title),
            React.createElement("div", { className: "sh-sub" }, t.sub)),
          React.createElement("button", { className: "tb-btn", onClick: onClose, "aria-label": "close" }, React.createElement(Icon, { name: "x", size: 16 }))),

        React.createElement("div", { className: "sync-body" },
          // changes
          React.createElement("div", null,
            React.createElement("div", { className: "sync-sec-label" }, t.changes, React.createElement("span", { className: "cnt" }, CHANGES.length)),
            CHANGES.length === 0
              ? React.createElement("div", { className: "chg-empty" }, t.noChanges)
              : React.createElement("div", { className: "chg-list" },
                  CHANGES.map((c, i) => React.createElement("div", { key: i, className: "chg-row" },
                    React.createElement("span", { className: "chg-badge " + c.s, title: t[c.s] }, c.s),
                    React.createElement("span", { className: "chg-path" }, React.createElement("span", { className: "dir" }, c.dir), c.name))))),

          // commit message + result
          React.createElement("div", null,
            React.createElement("input", { className: "commit-input", value: msg, placeholder: t.commitPh,
              onChange: (e) => setMsg(e.target.value), onKeyDown: (e) => { if (e.key === "Enter") commit(); } }),
            result && result.ok ? React.createElement("div", { className: "commit-result ok", style: { marginTop: 8 } },
              React.createElement(Icon, { name: "check", size: 15, className: "ico" }),
              React.createElement("span", null, t.committed(result.msg))) : null,
            result && result.blocked ? React.createElement("div", { className: "commit-result blocked", style: { marginTop: 8 } },
              React.createElement("div", { className: "cr-blocked-head" },
                React.createElement(Icon, { name: "shield-off", size: 15 }), t.secretsHead),
              React.createElement("div", { className: "cr-secrets" },
                SECRETS.map((s, i) => React.createElement("div", { key: i, className: "cr-secret" },
                  React.createElement("span", { className: "loc" }, s.loc),
                  React.createElement("span", { className: "kind" }, s.kind))))) : null),

          // remote (dashed)
          React.createElement("div", null,
            React.createElement("div", { className: "sync-sec-label" }, t.remote),
            React.createElement("div", { className: "remote-box" },
              React.createElement("div", { className: "field" },
                React.createElement("label", null, "Remote URL"),
                React.createElement("input", { className: "commit-input", value: remoteUrl, placeholder: t.remoteUrlPh, onChange: (e) => setRemoteUrl(e.target.value) })),
              React.createElement("div", { className: "field" },
                React.createElement("label", null, t.token),
                React.createElement("div", { className: "field-row" },
                  React.createElement("input", { className: "commit-input", type: "password", value: token, placeholder: t.tokenPh, onChange: (e) => setToken(e.target.value) }),
                  React.createElement("button", { className: "remote-connect", onClick: connect }, t.connect)),
                React.createElement("span", { className: "token-status " + (tokenSaved ? "has" : "no") },
                  React.createElement(Icon, { name: tokenSaved ? "check" : "key", size: 12, className: "ico" }),
                  tokenSaved ? t.tokenSaved : t.tokenNone)))),

          // sync status
          status ? React.createElement("div", { className: "sync-status " + status },
            React.createElement(Icon, { name: status === "conflict" ? "alert" : "check", size: 15, className: "ico" }),
            React.createElement("span", { className: "ss-text" }, status === "conflict" ? t.statusConflict : t.statusSynced),
            status === "conflict" ? React.createElement("button", { className: "ss-act", onClick: () => { onClose(); onResolveConflict && onResolveConflict(); } }, t.resolveConflicts) : null) : null),

        React.createElement("div", { className: "sync-foot" },
          React.createElement("button", { className: "sf-btn", disabled: !connected || busy, onClick: doSync },
            React.createElement(Icon, { name: "arrows-sync", size: 15 }), busy ? t.syncing : t.sync),
          React.createElement("button", { className: "sf-btn primary", disabled: !msg.trim() || busy, onClick: commit },
            React.createElement(Icon, { name: "git-merge", size: 15 }), busy ? t.committing : t.commit))));
  }
  window.SyncPanel = SyncPanel;
})();
