// onboarding.jsx — first-run flow: welcome → vault → AI check → indexing → enter.
(function () {
  const { useState, useEffect, useRef } = React;
  const Icon = window.Icon, BrandMark = window.BrandMark;

  const STR = {
    ru: {
      lang: "RU",
      welcome_eyebrow: "Добро пожаловать",
      welcome_title: "Nexus",
      welcome_sub: "Локальный редактор заметок со связями и встроенным AI. Ваши данные остаются на устройстве.",
      welcome_cta: "Начать настройку",
      welcome_foot: "Около минуты · 3 шага",

      vault_eyebrow: "Шаг 1 · Хранилище",
      vault_title: "Выберите хранилище",
      vault_sub: "Хранилище (vault) — это папка с вашими заметками в формате Markdown.",
      vault_open: "Открыть папку",
      vault_open_sub: "Выбрать существующую папку с заметками",
      vault_new: "Создать новое",
      vault_new_sub: "Пустое хранилище в выбранном месте",
      vault_demo: "Демо-хранилище",
      vault_demo_sub: "9 связанных заметок для знакомства",
      vault_recent: "Недавнее",

      ai_eyebrow: "Шаг 2 · AI",
      ai_title: "Подключение AI",
      ai_sub: "Nexus ищет по заметкам и отвечает со ссылками на источники. Выберите движок.",
      ai_local: "Локальная модель",
      ai_local_sub: "llama.cpp · приватно, без сети",
      ai_cloud: "Облачный провайдер",
      ai_cloud_sub: "Быстрее и точнее · требует ключ",
      ai_checking: "Проверка…",
      ai_online: "Готов",
      ai_offline: "Недоступен",
      ai_note: "Local-first: индекс и эмбеддинги хранятся на вашем устройстве. Облако используется только если вы явно его включите.",
      ai_skip: "Пока без AI",
      ai_cta: "Продолжить",

      idx_eyebrow: "Шаг 3 · Индексация",
      idx_title: "Индексация хранилища",
      idx_sub: "Строим векторный индекс для поиска и AI. Это разовая операция.",
      idx_done_title: "Готово",
      idx_done_sub: "Хранилище проиндексировано. Добро пожаловать в Nexus.",
      idx_chunks: "чанков",
      enter: "Открыть Nexus",
      back: "Назад",
    },
    en: {
      lang: "EN",
      welcome_eyebrow: "Welcome",
      welcome_title: "Nexus",
      welcome_sub: "A local-first notes editor with links and built-in AI. Your data stays on your device.",
      welcome_cta: "Get started",
      welcome_foot: "About a minute · 3 steps",

      vault_eyebrow: "Step 1 · Vault",
      vault_title: "Choose your vault",
      vault_sub: "A vault is a folder that holds your notes as Markdown files.",
      vault_open: "Open folder",
      vault_open_sub: "Pick an existing folder of notes",
      vault_new: "Create new",
      vault_new_sub: "An empty vault in a location you choose",
      vault_demo: "Demo vault",
      vault_demo_sub: "9 linked notes to explore",
      vault_recent: "Recent",

      ai_eyebrow: "Step 2 · AI",
      ai_title: "Connect AI",
      ai_sub: "Nexus searches your notes and answers with citations. Pick an engine.",
      ai_local: "Local model",
      ai_local_sub: "llama.cpp · private, offline",
      ai_cloud: "Cloud provider",
      ai_cloud_sub: "Faster and sharper · needs a key",
      ai_checking: "Checking…",
      ai_online: "Ready",
      ai_offline: "Unavailable",
      ai_note: "Local-first: your index and embeddings live on your device. The cloud is used only if you explicitly enable it.",
      ai_skip: "Skip AI for now",
      ai_cta: "Continue",

      idx_eyebrow: "Step 3 · Indexing",
      idx_title: "Indexing your vault",
      idx_sub: "Building the vector index for search and AI. This runs once.",
      idx_done_title: "All set",
      idx_done_sub: "Your vault is indexed. Welcome to Nexus.",
      idx_chunks: "chunks",
      enter: "Open Nexus",
      back: "Back",
    },
  };

  const FILES = ["Second Brain.md","RAG Pipeline.md","Embeddings.md","Nexus.md","Local-First.md",
    "Tauri Notes.md","Attention.md","HNSW Indexing.md","2026-06-02.md","Inbox.md","README.md"];

  function Steps({ step }) {
    const n = 3; // vault, ai, index (welcome is 0/intro)
    const cur = Math.max(0, step - 1);
    return React.createElement("div", { className: "onb-steps" },
      Array.from({ length: n }).map((_, i) =>
        React.createElement(React.Fragment, { key: i },
          i > 0 ? React.createElement("div", { className: "onb-step-line" + (i <= cur ? " filled" : "") }, React.createElement("i")) : null,
          React.createElement("div", { className: "onb-step-dot" + (i === cur ? " active" : "") + (i < cur ? " done" : "") }, React.createElement("span", { className: "dot" })))));
  }

  function Welcome({ t, onNext }) {
    return React.createElement("div", { className: "onb-card" },
      React.createElement("div", { className: "onb-mark-wrap" }, React.createElement(BrandMark, { size: 76, radius: 22 })),
      React.createElement("div", { className: "onb-eyebrow" }, t.welcome_eyebrow),
      React.createElement("h1", { className: "onb-title", style: { fontSize: 40, letterSpacing: "-0.03em" } }, t.welcome_title),
      React.createElement("p", { className: "onb-sub" }, t.welcome_sub),
      React.createElement("div", { className: "onb-actions" },
        React.createElement("button", { className: "btn btn-primary", onClick: onNext },
          t.welcome_cta, React.createElement(Icon, { name: "chevron", size: 16 }))),
      React.createElement("div", { className: "onb-foot-hint" }, React.createElement(Icon, { name: "clock", size: 14 }), t.welcome_foot));
  }

  function VaultStep({ t, value, setValue, onNext, onBack }) {
    const opts = [
      { id: "open", icon: "folder-open", title: t.vault_open, sub: "~/Documents/Notes", mono: true },
      { id: "new", icon: "plus", title: t.vault_new, sub: t.vault_new_sub },
      { id: "demo", icon: "sparkles", title: t.vault_demo, sub: t.vault_demo_sub },
    ];
    return React.createElement("div", { className: "onb-card left" },
      React.createElement("div", { className: "onb-eyebrow" }, t.vault_eyebrow),
      React.createElement("h1", { className: "onb-title" }, t.vault_title),
      React.createElement("p", { className: "onb-sub" }, t.vault_sub),
      React.createElement("div", { className: "onb-list" },
        opts.map((o) => React.createElement("button", { key: o.id, className: "onb-opt" + (value === o.id ? " sel" : ""), onClick: () => setValue(o.id) },
          React.createElement("span", { className: "oi" }, React.createElement(Icon, { name: o.icon, size: 18 })),
          React.createElement("span", { className: "ot" },
            React.createElement("span", { className: "ot-title" }, o.title),
            React.createElement("span", { className: "ot-sub" + (o.mono ? " mono" : "") }, o.sub)),
          value === o.id ? React.createElement("span", { className: "check" }, React.createElement(Icon, { name: "check", size: 13 })) : null))),
      React.createElement("div", { className: "onb-actions" },
        React.createElement("button", { className: "btn btn-text", onClick: onBack }, t.back),
        React.createElement("button", { className: "btn btn-primary", disabled: !value, onClick: onNext },
          t.ai_cta, React.createElement(Icon, { name: "chevron", size: 16 }))));
  }

  function HealthPill({ t, state }) {
    if (state === "checking") return React.createElement("span", { className: "health checking" },
      React.createElement(Icon, { name: "refresh", size: 12, className: "spin" }), t.ai_checking);
    if (state === "ok") return React.createElement("span", { className: "health ok" },
      React.createElement("span", { className: "live-dot" }), t.ai_online);
    return React.createElement("span", { className: "health bad" },
      React.createElement(Icon, { name: "alert", size: 12 }), t.ai_offline);
  }

  function AIStep({ t, value, setValue, onNext, onBack, onSkip }) {
    const [local, setLocal] = useState("checking");
    const [cloud, setCloud] = useState("checking");
    useEffect(() => {
      const a = setTimeout(() => setLocal("ok"), 1100);
      const b = setTimeout(() => setCloud("bad"), 1700);
      return () => { clearTimeout(a); clearTimeout(b); };
    }, []);
    return React.createElement("div", { className: "onb-card left" },
      React.createElement("div", { className: "onb-eyebrow" }, t.ai_eyebrow),
      React.createElement("h1", { className: "onb-title" }, t.ai_title),
      React.createElement("p", { className: "onb-sub" }, t.ai_sub),
      React.createElement("div", { className: "onb-list" },
        React.createElement("button", { className: "onb-opt" + (value === "local" ? " sel" : ""), onClick: () => local === "ok" && setValue("local"), disabled: local !== "ok" },
          React.createElement("span", { className: "oi" }, React.createElement(Icon, { name: "drive", size: 18 })),
          React.createElement("span", { className: "ot" },
            React.createElement("span", { className: "ot-title" }, t.ai_local, React.createElement(HealthPill, { t, state: local })),
            React.createElement("span", { className: "ot-sub mono" }, t.ai_local_sub)),
          value === "local" ? React.createElement("span", { className: "check" }, React.createElement(Icon, { name: "check", size: 13 })) : null),
        React.createElement("button", { className: "onb-opt" + (value === "cloud" ? " sel" : ""), onClick: () => cloud === "ok" && setValue("cloud"), disabled: cloud !== "ok" },
          React.createElement("span", { className: "oi" }, React.createElement(Icon, { name: "cloud", size: 18 })),
          React.createElement("span", { className: "ot" },
            React.createElement("span", { className: "ot-title" }, t.ai_cloud, React.createElement(HealthPill, { t, state: cloud })),
            React.createElement("span", { className: "ot-sub mono" }, t.ai_cloud_sub)),
          value === "cloud" ? React.createElement("span", { className: "check" }, React.createElement(Icon, { name: "check", size: 13 })) : null)),
      React.createElement("div", { className: "onb-note" },
        React.createElement(Icon, { name: "drive", size: 16, className: "ico" }), t.ai_note),
      React.createElement("div", { className: "onb-actions" },
        React.createElement("button", { className: "btn btn-text", onClick: onBack }, t.back),
        React.createElement("button", { className: "btn btn-ghost", onClick: onSkip }, t.ai_skip),
        React.createElement("button", { className: "btn btn-primary", disabled: !value, onClick: onNext },
          t.ai_cta, React.createElement(Icon, { name: "chevron", size: 16 }))));
  }

  function IndexStep({ t, onEnter }) {
    const TOTAL = 1200;
    const [done, setDone] = useState(0);
    const [files, setFiles] = useState([]);
    const complete = done >= TOTAL;
    useEffect(() => {
      let d = 0, fi = 0;
      const iv = setInterval(() => {
        d = Math.min(TOTAL, d + Math.round(40 + Math.random() * 70));
        setDone(d);
        if (fi < FILES.length && Math.random() > 0.4) { const cur = FILES[fi++]; setFiles((f) => [...f.slice(-5), cur]); }
        if (d >= TOTAL) clearInterval(iv);
      }, 130);
      return () => clearInterval(iv);
    }, []);
    const pct = Math.round(done / TOTAL * 100);
    return React.createElement("div", { className: "onb-card" },
      complete
        ? React.createElement(React.Fragment, null,
            React.createElement("div", { className: "onb-mark-wrap" },
              React.createElement("span", { className: "oi", style: { width: 64, height: 64, borderRadius: 20, background: "var(--color-accent)", color: "var(--color-on-accent)", display: "grid", placeItems: "center" } },
                React.createElement(Icon, { name: "check", size: 30, strokeWidth: 2.4 }))),
            React.createElement("h1", { className: "onb-title" }, t.idx_done_title),
            React.createElement("p", { className: "onb-sub" }, t.idx_done_sub),
            React.createElement("div", { className: "onb-actions" },
              React.createElement("button", { className: "btn btn-primary", onClick: onEnter, autoFocus: true },
                React.createElement(BrandMark, { size: 20, radius: 6 }), t.enter)))
        : React.createElement(React.Fragment, null,
            React.createElement("div", { className: "onb-eyebrow" }, t.idx_eyebrow),
            React.createElement("h1", { className: "onb-title" }, t.idx_title),
            React.createElement("p", { className: "onb-sub" }, t.idx_sub),
            React.createElement("div", { className: "onb-index-stat" }, done, " ", React.createElement("span", { className: "muted" }, "/ " + TOTAL + " " + t.idx_chunks)),
            React.createElement("div", { className: "onb-bigprog" }, React.createElement("i", { style: { width: pct + "%" } })),
            React.createElement("div", { className: "onb-files" },
              files.map((f, i) => React.createElement("div", { className: "onb-file", key: f + i },
                React.createElement(Icon, { name: "file-text", size: 13, className: "ico" }), f,
                React.createElement(Icon, { name: "check", size: 13, className: "tick" }))))));
  }

  function Onboarding() {
    const [step, setStep] = useState(0);
    const [lang, setLang] = useState(() => localStorage.getItem("nexus-lang") || "ru");
    const [theme, setTheme] = useState(() => localStorage.getItem("nexus-theme") || "light");
    const [vault, setVault] = useState("demo");
    const [ai, setAi] = useState("local");
    const t = STR[lang];

    useEffect(() => { document.documentElement.setAttribute("data-theme", theme); localStorage.setItem("nexus-theme", theme); }, [theme]);
    useEffect(() => { localStorage.setItem("nexus-lang", lang); }, [lang]);

    const enter = () => { localStorage.setItem("nexus-onboarded", "1"); window.location.href = "Nexus.html"; };

    return React.createElement("div", { className: "onb-root" },
      React.createElement("div", { className: "onb-bar" },
        React.createElement("div", { className: "traffic" },
          React.createElement("span", { className: "light r" }), React.createElement("span", { className: "light y" }), React.createElement("span", { className: "light g" })),
        React.createElement("div", { className: "spacer" }),
        React.createElement("button", { className: "tb-btn tb-lang", onClick: () => setLang(lang === "ru" ? "en" : "ru"), title: "RU / EN" },
          React.createElement("span", { className: lang === "ru" ? "on" : "" }, "RU"),
          React.createElement("span", { className: "sep" }, "/"),
          React.createElement("span", { className: lang === "en" ? "on" : "" }, "EN")),
        React.createElement("button", { className: "tb-btn", onClick: () => setTheme(theme === "dark" ? "light" : "dark"), title: "Theme" },
          React.createElement(Icon, { name: theme === "dark" ? "moon" : "sun", size: 16 }))),
      React.createElement("div", { className: "onb-stage" },
        step > 0 && step < 4 ? React.createElement(Steps, { step }) : null,
        React.createElement("div", { key: step },
          step === 0 ? React.createElement(Welcome, { t, onNext: () => setStep(1) }) : null,
          step === 1 ? React.createElement(VaultStep, { t, value: vault, setValue: setVault, onNext: () => setStep(2), onBack: () => setStep(0) }) : null,
          step === 2 ? React.createElement(AIStep, { t, value: ai, setValue: setAi, onNext: () => setStep(3), onBack: () => setStep(1), onSkip: () => { setAi(null); setStep(3); } }) : null,
          step === 3 ? React.createElement(IndexStep, { t, onEnter: enter }) : null)));
  }
  window.Onboarding = Onboarding;
})();
