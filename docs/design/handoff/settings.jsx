// settings.jsx — Settings modal (6 sections), wired to live app state.
(function () {
  const { useState } = React;
  const Icon = window.Icon;
  const Think = window.BrandThinking;

  const ACCENTS = [
    { id: "amber", h: 47, c: 0.135 }, { id: "teal", h: 205, c: 0.075 },
    { id: "sage", h: 158, c: 0.07 }, { id: "clay", h: 28, c: 0.11 },
  ];

  const T = {
    ru: {
      title: "Настройки",
      nav: { general: "Основное", editor: "Редактор", appearance: "Оформление", ai: "AI и модели", keys: "Горячие клавиши", about: "О программе" },
      generalT: "Основное", generalS: "Язык интерфейса и поведение приложения.",
      lang: "Язык интерфейса", langH: "Язык меню и панелей. Содержимое заметок не меняется.",
      autosave: "Автосохранение", autosaveH: "Сохранять правки автоматически по мере ввода.",
      confirmDelete: "Подтверждать удаление", confirmDeleteH: "Спрашивать перед удалением заметки.",
      editorT: "Редактор", editorS: "Типографика и поведение области письма.",
      font: "Шрифт текста", fontH: "Гарнитура для тела заметки.",
      defaultMode: "Режим по умолчанию", defaultModeH: "Как открывать заметку — просмотр или правка.",
      spellcheck: "Проверка орфографии", spellcheckH: "Подсветка ошибок в режиме правки.",
      appearanceT: "Оформление", appearanceS: "Тема, акцент и плотность интерфейса.",
      theme: "Тема", accent: "Акцент", density: "Плотность",
      aiT: "AI и модели", aiS: "Локальные и облачные эндпоинты. Данные не покидают устройство без явного облака.",
      embed: "Модель эмбеддингов", chatModel: "Модель чата", endpoint: "Эндпоинт", apiKey: "API-ключ (опц.)",
      test: "Проверить связь", testing: "Проверяю…", connected: "Подключено", failed: "Недоступно",
      keysT: "Горячие клавиши", keysS: "Основные сочетания. Кликабельны в реальном приложении.",
      aboutT: "О программе",
      preview: "Просмотр", edit: "Правка", compact: "Плотно", comfortable: "Свободно",
      reset: "Сбросить настройки",
    },
    en: {
      title: "Settings",
      nav: { general: "General", editor: "Editor", appearance: "Appearance", ai: "AI & models", keys: "Shortcuts", about: "About" },
      generalT: "General", generalS: "Interface language and app behavior.",
      lang: "Interface language", langH: "Language of menus and panels. Note content is unchanged.",
      autosave: "Autosave", autosaveH: "Save edits automatically as you type.",
      confirmDelete: "Confirm deletion", confirmDeleteH: "Ask before deleting a note.",
      editorT: "Editor", editorS: "Typography and writing-area behavior.",
      font: "Body font", fontH: "Typeface for the note body.",
      defaultMode: "Default mode", defaultModeH: "Open notes in preview or edit.",
      spellcheck: "Spellcheck", spellcheckH: "Highlight mistakes in edit mode.",
      appearanceT: "Appearance", appearanceS: "Theme, accent and interface density.",
      theme: "Theme", accent: "Accent", density: "Density",
      aiT: "AI & models", aiS: "Local and cloud endpoints. Data stays on device unless you opt into cloud.",
      embed: "Embedding model", chatModel: "Chat model", endpoint: "Endpoint", apiKey: "API key (opt.)",
      test: "Test connection", testing: "Testing…", connected: "Connected", failed: "Unavailable",
      keysT: "Shortcuts", keysS: "Core combos. Editable in the real app.",
      aboutT: "About",
      preview: "Preview", edit: "Edit", compact: "Compact", comfortable: "Comfortable",
      reset: "Reset settings",
    },
  };

  function Row({ name, hint, children }) {
    return React.createElement("div", { className: "set-row" },
      React.createElement("div", { className: "sr-label" },
        React.createElement("div", { className: "sr-name" }, name),
        hint ? React.createElement("div", { className: "sr-hint" }, hint) : null),
      React.createElement("div", { className: "sr-control" }, children));
  }
  function Seg({ value, options, onChange }) {
    return React.createElement("div", { className: "set-seg" },
      options.map((o) => React.createElement("button", { key: o.v, className: o.v === value ? "on" : "", onClick: () => onChange(o.v) }, o.label)));
  }
  function Switch({ on, onChange }) {
    return React.createElement("div", { className: "set-switch" + (on ? " on" : ""), role: "switch", "aria-checked": on, tabIndex: 0, onClick: () => onChange(!on) }, React.createElement("i", null));
  }

  function Settings(props) {
    const { lang, setLang, theme, setTheme, accent, setAccent, density, setDensity, editorFont, setEditorFont, toast, onClose } = props;
    const t = T[lang] || T.en;
    const [sec, setSec] = useState("general");
    const [autosave, setAutosave] = useState(true);
    const [confirmDel, setConfirmDel] = useState(true);
    const [spell, setSpell] = useState(false);
    const [defMode, setDefMode] = useState("preview");
    const [testState, setTestState] = useState({ embed: null, chat: null }); // null|testing|ok|bad

    function test(which, willFail) {
      setTestState((s) => ({ ...s, [which]: "testing" }));
      setTimeout(() => setTestState((s) => ({ ...s, [which]: willFail ? "bad" : "ok" })), 1300);
    }
    function statusEl(st) {
      if (!st) return null;
      if (st === "testing") return React.createElement("span", { className: "set-status testing" }, React.createElement(Think, { size: 13 }), t.testing);
      if (st === "ok") return React.createElement("span", { className: "set-status ok" }, React.createElement("span", { className: "dot" }), t.connected);
      return React.createElement("span", { className: "set-status bad" }, React.createElement("span", { className: "dot" }), t.failed);
    }

    const NAV = [
      ["general", "sliders"], ["editor", "file-text"], ["appearance", "palette"],
      ["ai", "cpu"], ["keys", "keyboard"], ["about", "info"],
    ];

    function body() {
      if (sec === "general") return React.createElement("div", null,
        React.createElement("div", { className: "set-sec-title" }, t.generalT),
        React.createElement("div", { className: "set-sec-sub" }, t.generalS),
        React.createElement(Row, { name: t.lang, hint: t.langH },
          React.createElement(Seg, { value: lang, options: [{ v: "ru", label: "RU" }, { v: "en", label: "EN" }], onChange: setLang })),
        React.createElement(Row, { name: t.autosave, hint: t.autosaveH }, React.createElement(Switch, { on: autosave, onChange: setAutosave })),
        React.createElement(Row, { name: t.confirmDelete, hint: t.confirmDeleteH }, React.createElement(Switch, { on: confirmDel, onChange: setConfirmDel })));

      if (sec === "editor") return React.createElement("div", null,
        React.createElement("div", { className: "set-sec-title" }, t.editorT),
        React.createElement("div", { className: "set-sec-sub" }, t.editorS),
        React.createElement(Row, { name: t.font, hint: t.fontH },
          React.createElement(Seg, { value: editorFont, options: [{ v: "sans", label: "Sans" }, { v: "serif", label: "Serif" }, { v: "mono", label: "Mono" }], onChange: setEditorFont })),
        React.createElement(Row, { name: t.defaultMode, hint: t.defaultModeH },
          React.createElement(Seg, { value: defMode, options: [{ v: "preview", label: t.preview }, { v: "edit", label: t.edit }], onChange: setDefMode })),
        React.createElement(Row, { name: t.spellcheck, hint: t.spellcheckH }, React.createElement(Switch, { on: spell, onChange: setSpell })));

      if (sec === "appearance") return React.createElement("div", null,
        React.createElement("div", { className: "set-sec-title" }, t.appearanceT),
        React.createElement("div", { className: "set-sec-sub" }, t.appearanceS),
        React.createElement(Row, { name: t.theme },
          React.createElement(Seg, { value: theme, options: [{ v: "light", label: "☀" }, { v: "dark", label: "☾" }, { v: "midnight", label: "✦" }, { v: "platinum", label: "◇" }], onChange: setTheme })),
        React.createElement(Row, { name: t.accent },
          React.createElement("div", { className: "set-accents" },
            ACCENTS.map((a) => React.createElement("button", { key: a.id, title: a.id,
              className: accent === a.id ? "on" : "", style: { background: "oklch(0.6 " + a.c + " " + a.h + ")" },
              onClick: () => setAccent(a.id) })))),
        React.createElement(Row, { name: t.density },
          React.createElement(Seg, { value: density, options: [{ v: "compact", label: t.compact }, { v: "comfortable", label: t.comfortable }], onChange: setDensity })));

      if (sec === "ai") return React.createElement("div", null,
        React.createElement("div", { className: "set-sec-title" }, t.aiT),
        React.createElement("div", { className: "set-sec-sub" }, t.aiS),
        React.createElement("div", { className: "set-model-card" },
          React.createElement("div", { className: "smc-head" }, React.createElement(Icon, { name: "drive", size: 16 }), React.createElement("span", { className: "smc-title" }, t.embed), React.createElement("span", { className: "provider local" }, React.createElement(Icon, { name: "drive", size: 12, className: "ico" }), "local")),
          React.createElement("div", { className: "set-field" }, React.createElement("label", null, t.endpoint), React.createElement("input", { className: "set-input mono", defaultValue: "http://localhost:11434", spellCheck: false })),
          React.createElement("div", { className: "set-field" }, React.createElement("label", null, "Model"), React.createElement("input", { className: "set-input mono", defaultValue: "nomic-embed-text", spellCheck: false })),
          React.createElement("div", null, React.createElement("button", { className: "set-test", onClick: () => test("embed", false) }, React.createElement(Icon, { name: "cpu", size: 14 }), t.test), statusEl(testState.embed))),
        React.createElement("div", { className: "set-model-card" },
          React.createElement("div", { className: "smc-head" }, React.createElement(Icon, { name: "sparkles", size: 16 }), React.createElement("span", { className: "smc-title" }, t.chatModel), React.createElement("span", { className: "provider local" }, React.createElement(Icon, { name: "drive", size: 12, className: "ico" }), "local")),
          React.createElement("div", { className: "set-field" }, React.createElement("label", null, t.endpoint), React.createElement("input", { className: "set-input mono", defaultValue: "http://localhost:11434", spellCheck: false })),
          React.createElement("div", { className: "set-field" }, React.createElement("label", null, "Model"), React.createElement("input", { className: "set-input mono", defaultValue: "qwen3:35b", spellCheck: false })),
          React.createElement("div", { className: "set-field" }, React.createElement("label", null, t.apiKey), React.createElement("input", { className: "set-input mono", type: "password", defaultValue: "", placeholder: "—", spellCheck: false })),
          React.createElement("div", null, React.createElement("button", { className: "set-test", onClick: () => test("chat", false) }, React.createElement(Icon, { name: "cpu", size: 14 }), t.test), statusEl(testState.chat))));

      if (sec === "keys") {
        const KEYS = [
          ["Командная палитра / Command palette", ["⌘", "K"]], ["Граф / Graph", ["⌘", "⇧", "G"]],
          ["Разделить / Split", ["⌘", "\\"]], ["Режим чтения / Reading", ["⌘", "R"]],
          ["Правка ↔ Просмотр / Edit ↔ Preview", ["⌘", "E"]], ["Сохранить / Save", ["⌘", "S"]],
          ["Настройки / Settings", ["⌘", ","]], ["Новая заметка / New note", ["⌘", "N"]],
        ];
        return React.createElement("div", null,
          React.createElement("div", { className: "set-sec-title" }, t.keysT),
          React.createElement("div", { className: "set-sec-sub" }, t.keysS),
          React.createElement("div", { className: "set-keys" },
            KEYS.map(([label, combo], i) => React.createElement("div", { key: i, className: "set-key-row" },
              React.createElement("span", { className: "kk-label" }, label),
              React.createElement("span", { className: "kk-combo" }, combo.map((k, j) => React.createElement("span", { key: j, className: "kbd" }, k)))))));
      }

      if (sec === "about") return React.createElement("div", { className: "set-about" },
        React.createElement(window.BrandMark, { size: 56, radius: 16 }),
        React.createElement("div", { className: "sa-name" }, "Nexus"),
        React.createElement("div", { className: "sa-ver" }, "v0.9.0 · build 2026.06.09"),
        React.createElement("div", { className: "sa-row" },
          React.createElement("span", { className: "sa-link" }, lang === "ru" ? "Сайт" : "Website"),
          React.createElement("span", { className: "sa-link" }, lang === "ru" ? "Документация" : "Docs"),
          React.createElement("span", { className: "sa-link" }, "GitHub")),
        React.createElement("div", { className: "sa-meta" }, lang === "ru"
          ? "Local-first редактор знаний с AI-слоем.\nTauri · React · Rust. Данные хранятся на вашем устройстве."
          : "Local-first knowledge editor with an AI layer.\nTauri · React · Rust. Your data stays on your device."),
        React.createElement("button", { className: "set-test", style: { marginTop: 16 }, onClick: () => toast && toast(lang === "ru" ? "Настройки сброшены" : "Settings reset") },
          React.createElement(Icon, { name: "rotate-ccw", size: 14 }), t.reset));
      return null;
    }

    return React.createElement("div", { className: "set-scrim", onMouseDown: (e) => { if (e.target === e.currentTarget) onClose(); } },
      React.createElement("div", { className: "set-panel", role: "dialog", "aria-label": t.title },
        React.createElement("div", { className: "set-head" },
          React.createElement("div", { className: "sh-ic" }, React.createElement(Icon, { name: "settings", size: 17 })),
          React.createElement("div", { className: "sh-title" }, t.title),
          React.createElement("button", { className: "tb-btn", onClick: onClose, "aria-label": "close" }, React.createElement(Icon, { name: "x", size: 16 }))),
        React.createElement("div", { className: "set-nav" },
          NAV.map(([id, ico]) => React.createElement("div", { key: id, className: "sn-item" + (sec === id ? " active" : ""), onClick: () => setSec(id) },
            React.createElement(Icon, { name: ico, size: 15, className: "ico" }), t.nav[id]))),
        React.createElement("div", { className: "set-main" }, body())));
  }
  window.Settings = Settings;
})();
