// plugins.jsx — plugin manager + permission consent sheet.
(function () {
  const { useState } = React;
  const Icon = window.Icon;

  const STR = {
    ru: {
      title: "Менеджер плагинов", sub: "Расширения работают в песочнице с явными разрешениями.",
      installed: "Установленные", browse: "Маркетплейс",
      privacy: "Плагины не получают доступ к сети или файлам, пока вы явно не разрешите.",
      enable: "Включить", install: "Установить", installed_b: "Установлено", remove: "Удалить",
      by: "от", requests: "запрашивает разрешения", grant_sub: (n) => `«${n}» хочет получить доступ к:`,
      allow: "Разрешить и включить", cancel: "Отмена",
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
      title: "Plugin manager", sub: "Extensions run sandboxed with explicit permissions.",
      installed: "Installed", browse: "Marketplace",
      privacy: "Plugins get no network or file access until you explicitly allow it.",
      enable: "Enable", install: "Install", installed_b: "Installed", remove: "Remove",
      by: "by", requests: "requests permissions", grant_sub: (n) => `“${n}” wants access to:`,
      allow: "Allow and enable", cancel: "Cancel",
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

  function PermChip({ t, scope }) {
    const [label, , level] = t.perms[scope];
    return React.createElement("span", { className: "perm-chip" + (level === "sensitive" ? " sensitive" : level === "caution" ? " caution" : "") },
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
      const s = {}; PLUGINS.forEach((p) => (s[p.id] = { installed: p.installed, enabled: p.enabled })); return s;
    });
    const [consent, setConsent] = useState(null); // plugin awaiting consent

    function attempt(p) {
      // turning ON / installing → if it has sensitive/caution perms, ask consent
      const needs = p.perms.some((scope) => t.perms[scope][2] !== "safe");
      if (needs) { setConsent(p); return; }
      grant(p);
    }
    function grant(p) {
      setState((s) => ({ ...s, [p.id]: { installed: true, enabled: true } }));
      setConsent(null);
      toast && toast((lang === "ru" ? "Включён · " : "Enabled · ") + p.name);
    }
    function disable(p) { setState((s) => ({ ...s, [p.id]: { ...s[p.id], enabled: false } })); }

    const list = PLUGINS.filter((p) => tab === "installed" ? state[p.id].installed : !state[p.id].installed);
    const instCount = PLUGINS.filter((p) => state[p.id].installed).length;

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
          React.createElement("div", { className: "nav-note" },
            React.createElement(Icon, { name: "shield-check", size: 15, className: "ico" }), t.privacy)),
        React.createElement("div", { className: "plg-main" },
          list.map((p) => {
            const st = state[p.id];
            return React.createElement("div", { key: p.id, className: "plg-card" },
              React.createElement("div", { className: "plg-glyph" + (st.enabled ? " on" : "") }, React.createElement(Icon, { name: p.glyph, size: 22 })),
              React.createElement("div", { className: "plg-body" },
                React.createElement("div", { className: "plg-name" }, p.name,
                  React.createElement("span", { className: "plg-ver" }, "v" + p.ver),
                  React.createElement("span", { className: "plg-author" }, t.by + " " + p.author)),
                React.createElement("div", { className: "plg-desc" }, p.desc[lang] || p.desc.en),
                React.createElement("div", { className: "plg-perms" }, p.perms.map((s) => React.createElement(PermChip, { key: s, t, scope: s })))),
              React.createElement("div", { className: "plg-side" },
                st.installed
                  ? React.createElement(React.Fragment, null,
                      React.createElement("div", { className: "switch" + (st.enabled ? " on" : ""), role: "switch", "aria-checked": st.enabled, tabIndex: 0,
                        onClick: () => st.enabled ? disable(p) : attempt(p) }, React.createElement("div", { className: "knob" })),
                      React.createElement("button", { className: "plg-remove", onClick: () => setState((s) => ({ ...s, [p.id]: { installed: false, enabled: false } })) },
                        React.createElement(Icon, { name: "trash", size: 12 }), t.remove))
                  : React.createElement("button", { className: "plg-install", onClick: () => attempt(p) },
                      React.createElement(Icon, { name: "download", size: 14 }), t.install)));
          })),
        consent ? React.createElement(Consent, { t, plugin: consent, onAllow: () => grant(consent), onCancel: () => setConsent(null) }) : null));
  }
  window.PluginManager = PluginManager;
})();
