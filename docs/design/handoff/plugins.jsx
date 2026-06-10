// plugins.jsx — plugin manager + permission consent sheet.
(function () {
  const { useState } = React;
  const Icon = window.Icon;

  const STR = {
    ru: {
      title: "Менеджер плагинов", sub: "Каждый плагин изолирован в песочнице-iframe. Доступы — по запросу.",
      installed: "Установленные", browse: "Маркетплейс", audit: "Журнал доступа",
      privacy: "Плагины не получают доступ к сети или файлам, пока вы явно не разрешите.",
      enable: "Включить", install: "Установить", installed_b: "Установлено", remove: "Удалить",
      by: "от", requests: "запрашивает разрешения", grant_sub: (n) => `«${n}» хочет получить доступ к:`,
      allow: "Разрешить и включить", cancel: "Отмена",
      sandbox: "Sandbox-iframe", runtime: "Разрешения", revoke: "Отозвать", granted: "Разрешено", denied: "Запрещено",
      manage: "Управление доступом", auditEmpty: "Пока нет событий", auditSub: "Все обращения плагинов к API логируются.",
      consent_note: "Можно отозвать в любой момент в настройках плагина",
      perms: {
        readVault: ["Чтение заметок", "Видит содержимое вашего хранилища", "safe"],
        writeVault: ["Изменение заметок", "Может создавать и править файлы в хранилище", "caution"],
        network: ["Доступ к сети", "Может отправлять и получать данные из интернета", "sensitive"],
        clipboard: ["Буфер обмена", "Чтение и запись буфера обмена", "caution"],
        shell: ["Запуск команд", "Выполняет команды в системе", "sensitive"],
        fsOut: ["Файлы вне хранилища", "Доступ к файлам за пределами vault", "sensitive"],
      },
    },
    en: {
      title: "Plugin manager", sub: "Each plugin is isolated in a sandbox iframe. Access is request-based.",
      installed: "Installed", browse: "Marketplace", audit: "Access log",
      privacy: "Plugins get no network or file access until you explicitly allow it.",
      enable: "Enable", install: "Install", installed_b: "Installed", remove: "Remove",
      by: "by", requests: "requests permissions", grant_sub: (n) => `“${n}” wants access to:`,
      allow: "Allow and enable", cancel: "Cancel",
      sandbox: "Sandboxed iframe", runtime: "Permissions", revoke: "Revoke", granted: "Granted", denied: "Denied",
      manage: "Manage access", auditEmpty: "No events yet", auditSub: "Every plugin API call is logged.",
      consent_note: "Revocable anytime in the plugin's settings",
      perms: {
        readVault: ["Read notes", "Can see the contents of your vault", "safe"],
        writeVault: ["Modify notes", "Can create and edit files in the vault", "caution"],
        network: ["Network access", "Can send and receive data over the internet", "sensitive"],
        clipboard: ["Clipboard", "Read and write your clipboard", "caution"],
        shell: ["Run commands", "Executes commands on your system", "sensitive"],
        fsOut: ["Files outside vault", "Access files beyond the vault folder", "sensitive"],
      },
    },
  };

  const PERM_ICON = { readVault: "file-text", writeVault: "file", network: "globe", clipboard: "clipboard", shell: "terminal", fsOut: "folder" };

  // recent sandbox audit events (mock) — plugin → permission → when
  const AUDIT = [
    { plugin: "Git Sync", scope: "shell", action: { ru: "git commit -m \"sync\"", en: "git commit -m \"sync\"" }, when: { ru: "2 мин назад", en: "2 min ago" }, ok: true },
    { plugin: "Git Sync", scope: "network", action: { ru: "push → github.com", en: "push → github.com" }, when: { ru: "2 мин назад", en: "2 min ago" }, ok: true },
    { plugin: "Mermaid Diagrams", scope: "readVault", action: { ru: "чтение «RAG Pipeline.md»", en: "read “RAG Pipeline.md”" }, when: { ru: "14 мин назад", en: "14 min ago" }, ok: true },
    { plugin: "Web Clipper", scope: "network", action: { ru: "запрос заблокирован (нет доступа)", en: "request blocked (no grant)" }, when: { ru: "1 ч назад", en: "1 h ago" }, ok: false },
    { plugin: "Git Sync", scope: "writeVault", action: { ru: "запись .obsidian/workspace", en: "wrote .obsidian/workspace" }, when: { ru: "1 ч назад", en: "1 h ago" }, ok: true },
  ];

  const PLUGINS = [
    { id: "git", name: "Git Sync", author: "nexus-labs", ver: "2.4.0", glyph: "git-merge", installed: true, enabled: true, perms: ["readVault","writeVault","network","shell"],
      desc: { ru: "Версионирование хранилища через Git, авто-коммиты и пуш.", en: "Version your vault with Git — auto-commit and push." } },
    { id: "mermaid", name: "Mermaid Diagrams", author: "community", ver: "1.8.2", glyph: "graph", installed: true, enabled: true, perms: ["readVault"],
      desc: { ru: "Рендер диаграмм Mermaid прямо в заметках.", en: "Render Mermaid diagrams inline in your notes." } },
    { id: "clipper", name: "Web Clipper", author: "nexus-labs", ver: "0.9.1", glyph: "download", installed: true, enabled: false, perms: ["network","writeVault","clipboard"],
      desc: { ru: "Сохраняет веб-страницы в заметки в формате Markdown.", en: "Save web pages into notes as clean Markdown." } },
    { id: "calendar", name: "Calendar", author: "community", ver: "3.1.0", glyph: "clock", installed: false, enabled: false, perms: ["readVault","writeVault"],
      desc: { ru: "Календарь по ежедневным заметкам с переходами.", en: "A calendar view over your daily notes." } },
    { id: "translate", name: "AI Translator", author: "community", ver: "1.2.4", glyph: "languages", installed: false, enabled: false, perms: ["readVault","network"],
      desc: { ru: "Перевод выделенного текста через облачную модель.", en: "Translate selected text via a cloud model." } },
    { id: "canvas", name: "Excalidraw", author: "community", ver: "5.0.0", glyph: "puzzle", installed: false, enabled: false, perms: ["readVault","writeVault"],
      desc: { ru: "Рисованные схемы и доски прямо в хранилище.", en: "Hand-drawn diagrams and boards inside your vault." } },
  ];

  function PermChip({ t, scope, on }) {
    const [label, , level] = t.perms[scope];
    const revoked = on === false;
    return React.createElement("span", { className: "perm-chip" + (level === "sensitive" ? " sensitive" : level === "caution" ? " caution" : "") + (revoked ? " revoked" : ""), title: revoked ? t.denied : (on === true ? t.granted : "") },
      React.createElement(Icon, { name: PERM_ICON[scope], size: 11 }), label);
  }

  function Consent({ t, plugin, onAllow, onCancel }) {
    return React.createElement("div", { className: "consent-wrap", onMouseDown: (e) => { if (e.target === e.currentTarget) onCancel(); } },
      React.createElement("div", { className: "consent", role: "dialog", "aria-label": t.requests },
        React.createElement("div", { className: "consent-top" },
          React.createElement("div", { className: "c-glyph" }, React.createElement(Icon, { name: plugin.glyph, size: 26 })),
          React.createElement("div", { className: "c-title" }, plugin.name),
          React.createElement("div", { className: "c-sub" }, React.createElement("b", null, plugin.name), " " + t.requests)),
        React.createElement("div", { className: "perm-rows" },
          plugin.perms.map((scope) => {
            const [title, desc, level] = t.perms[scope];
            return React.createElement("div", { key: scope, className: "perm-row " + level },
              React.createElement("div", { className: "pr-ic" }, React.createElement(Icon, { name: PERM_ICON[scope], size: 16 })),
              React.createElement("div", { className: "pr-tt" },
                React.createElement("div", { className: "pr-title" }, title,
                  level !== "safe" ? React.createElement("span", { className: "pr-badge" }, level === "sensitive" ? "!" : "~") : null),
                React.createElement("div", { className: "pr-desc" }, desc)));
          })),
        React.createElement("div", { className: "consent-foot" },
          React.createElement("button", { className: "btn btn-text", onClick: onCancel }, t.cancel),
          React.createElement("button", { className: "btn btn-primary", onClick: onAllow, autoFocus: true },
            React.createElement(Icon, { name: "shield-check", size: 16 }), t.allow)),
        React.createElement("div", { className: "consent-note" },
          React.createElement(Icon, { name: "shield", size: 12 }), t.consent_note)));
  }

  function PluginManager({ lang, onClose, toast }) {
    const t = STR[lang] || STR.en;
    const [tab, setTab] = useState("installed");
    const [state, setState] = useState(() => {
      const s = {}; PLUGINS.forEach((p) => (s[p.id] = { installed: p.installed, enabled: p.enabled,
        grants: Object.fromEntries(p.perms.map((sc) => [sc, p.installed && p.enabled])) })); return s;
    });
    const [consent, setConsent] = useState(null); // plugin awaiting consent
    const [detail, setDetail] = useState(null);    // plugin id whose runtime perms are open

    function attempt(p) {
      const needs = p.perms.some((scope) => t.perms[scope][2] !== "safe");
      if (needs) { setConsent(p); return; }
      grant(p);
    }
    function grant(p) {
      setState((s) => ({ ...s, [p.id]: { installed: true, enabled: true, grants: Object.fromEntries(p.perms.map((sc) => [sc, true])) } }));
      setConsent(null);
      toast && toast((lang === "ru" ? "Включён · " : "Enabled · ") + p.name);
    }
    function disable(p) { setState((s) => ({ ...s, [p.id]: { ...s[p.id], enabled: false } })); }
    function toggleGrant(p, scope) {
      setState((s) => {
        const g = { ...s[p.id].grants, [scope]: !s[p.id].grants[scope] };
        return { ...s, [p.id]: { ...s[p.id], grants: g } };
      });
      const nowOn = !state[p.id].grants[scope];
      toast && toast((nowOn ? (lang === "ru" ? "Разрешено: " : "Granted: ") : (lang === "ru" ? "Отозвано: " : "Revoked: ")) + t.perms[scope][0]);
    }

    const list = PLUGINS.filter((p) => tab === "installed" ? state[p.id].installed : !state[p.id].installed);
    const instCount = PLUGINS.filter((p) => state[p.id].installed).length;
    const detailPlugin = detail ? PLUGINS.find((p) => p.id === detail) : null;

    function renderCard(p) {
      const st = state[p.id];
      const isOpen = detail === p.id;
      return React.createElement("div", { key: p.id, className: "plg-card" + (isOpen ? " open" : "") },
        React.createElement("div", { className: "plg-card-main" },
          React.createElement("div", { className: "plg-glyph" + (st.enabled ? " on" : "") }, React.createElement(Icon, { name: p.glyph, size: 22 })),
          React.createElement("div", { className: "plg-body" },
            React.createElement("div", { className: "plg-name" }, p.name,
              React.createElement("span", { className: "plg-ver" }, "v" + p.ver),
              React.createElement("span", { className: "plg-author" }, t.by + " " + p.author),
              React.createElement("span", { className: "plg-sandbox", title: t.sandbox },
                React.createElement(Icon, { name: "shield", size: 10 }), "sandbox")),
            React.createElement("div", { className: "plg-desc" }, p.desc[lang] || p.desc.en),
            React.createElement("div", { className: "plg-perms" },
              p.perms.map((s) => React.createElement(PermChip, { key: s, t, scope: s, on: st.installed ? st.grants[s] : null })),
              st.installed && st.enabled ? React.createElement("button", { className: "plg-manage", onClick: () => setDetail(isOpen ? null : p.id) },
                React.createElement(Icon, { name: "sliders", size: 11 }), t.manage) : null)),
          React.createElement("div", { className: "plg-side" },
            st.installed
              ? React.createElement(React.Fragment, null,
                  React.createElement("div", { className: "switch" + (st.enabled ? " on" : ""), role: "switch", "aria-checked": st.enabled, tabIndex: 0,
                    onClick: () => st.enabled ? disable(p) : attempt(p) }, React.createElement("div", { className: "knob" })),
                  React.createElement("button", { className: "plg-remove", onClick: () => setState((s) => ({ ...s, [p.id]: { installed: false, enabled: false, grants: {} } })) },
                    React.createElement(Icon, { name: "trash", size: 12 }), t.remove))
              : React.createElement("button", { className: "plg-install", onClick: () => attempt(p) },
                  React.createElement(Icon, { name: "download", size: 14 }), t.install))),
        isOpen ? React.createElement("div", { className: "plg-runtime" },
          React.createElement("div", { className: "plg-runtime-head" }, React.createElement(Icon, { name: "shield-check", size: 13 }), t.runtime),
          p.perms.map((scope) => {
            const [label, desc, level] = t.perms[scope];
            const on = st.grants[scope];
            return React.createElement("div", { key: scope, className: "rt-row" },
              React.createElement("div", { className: "rt-ic " + level }, React.createElement(Icon, { name: PERM_ICON[scope], size: 14 })),
              React.createElement("div", { className: "rt-tt" },
                React.createElement("div", { className: "rt-name" }, label),
                React.createElement("div", { className: "rt-desc" }, desc)),
              React.createElement("div", { className: "switch sm" + (on ? " on" : ""), role: "switch", "aria-checked": on, tabIndex: 0,
                onClick: () => toggleGrant(p, scope) }, React.createElement("div", { className: "knob" })));
          })) : null);
    }

    function renderAudit() {
      return React.createElement("div", { className: "plg-audit" },
        React.createElement("div", { className: "plg-audit-sub" }, t.auditSub),
        AUDIT.map((e, i) => React.createElement("div", { key: i, className: "audit-row" + (e.ok ? "" : " blocked") },
          React.createElement("div", { className: "audit-ic " + t.perms[e.scope][2] }, React.createElement(Icon, { name: e.ok ? PERM_ICON[e.scope] : "x", size: 13 })),
          React.createElement("div", { className: "audit-tt" },
            React.createElement("div", { className: "audit-line" },
              React.createElement("span", { className: "audit-plugin" }, e.plugin),
              React.createElement("span", { className: "audit-scope" }, t.perms[e.scope][0])),
            React.createElement("div", { className: "audit-action" }, e.action[lang] || e.action.en)),
          React.createElement("div", { className: "audit-when" }, e.when[lang] || e.when.en))));
    }

    return React.createElement("div", { className: "plg-scrim", onMouseDown: (e) => { if (e.target === e.currentTarget) onClose(); } },
      React.createElement("div", { className: "plg-panel", role: "dialog", "aria-label": t.title },
        React.createElement("div", { className: "plg-head" },
          React.createElement("div", { className: "plg-ic" }, React.createElement(Icon, { name: "puzzle", size: 19 })),
          React.createElement("div", { className: "h-tt" },
            React.createElement("div", { className: "h-title" }, t.title),
            React.createElement("div", { className: "h-sub" }, t.sub)),
          React.createElement("button", { className: "tb-btn", onClick: onClose, "aria-label": "close" }, React.createElement(Icon, { name: "x", size: 16 }))),
        React.createElement("div", { className: "plg-nav" },
          React.createElement("div", { className: "nav-item" + (tab === "installed" ? " active" : ""), onClick: () => setTab("installed") },
            React.createElement(Icon, { name: "puzzle", size: 16 }), t.installed, React.createElement("span", { className: "cnt" }, instCount)),
          React.createElement("div", { className: "nav-item" + (tab === "browse" ? " active" : ""), onClick: () => setTab("browse") },
            React.createElement(Icon, { name: "download", size: 16 }), t.browse, React.createElement("span", { className: "cnt" }, PLUGINS.length - instCount)),
          React.createElement("div", { className: "nav-item" + (tab === "audit" ? " active" : ""), onClick: () => setTab("audit") },
            React.createElement(Icon, { name: "clock", size: 16 }), t.audit),
          React.createElement("div", { className: "nav-note" },
            React.createElement(Icon, { name: "shield-check", size: 15, className: "ico" }), t.privacy)),
        React.createElement("div", { className: "plg-main" },
          tab === "audit" ? renderAudit() : list.map(renderCard)),
        consent ? React.createElement(Consent, { t, plugin: consent, onAllow: () => grant(consent), onCancel: () => setConsent(null) }) : null));
  }
  window.PluginManager = PluginManager;
})();
