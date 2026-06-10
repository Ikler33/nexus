// data.jsx — i18n strings (ru/en) + mock vault, note bodies, chat scaffolding.
(function () {
  // ---------------- i18n ----------------
  const STR = {
    ru: {
      search_vault: "Поиск по vault…",
      search_files_cmds: "Поиск файлов и команд…",
      explorer: "Файлы", search: "Поиск", tags: "Теги", starred: "Избранное",
      ai_assistant: "AI-ассистент", chat: "Чат", suggestions: "Связи", summary: "Резюме",
      backlinks: "Обратные ссылки", no_backlinks: "Нет обратных ссылок",
      backlinks_count: (n) => `${n} ${plural(n, ["обратная ссылка","обратные ссылки","обратных ссылок"])}`,
      mentioned_in: "Упоминается в",
      ask_anything: "Спросите о ваших заметках…",
      ai_empty_title: "О чём спросить?",
      ai_empty_sub: "RAG по локальному индексу. Ответы со ссылками на источники.",
      stop: "Стоп", thinking: "Думаю…", sources: "Источники",
      local: "локально", cloud: "облако", cloud_answer: "Ответ получен из облака",
      indexing: (a,b) => `Индексация ${a}/${b} чанков`, indexed: "Проиндексировано",
      llm_offline: "LLM недоступен", llm_offline_sub: "Проверьте настройки серверов",
      degraded_fts: "Семантика недоступна — только полнотекстовый поиск",
      reindex_banner: "Сменилась модель — переиндексация векторов",
      recompute: "Пересчитать", accept: "Принять", dismiss: "Скрыть",
      new_tab: "Новая вкладка", close: "Закрыть", split: "Разделить",
      no_results: "Ничего не найдено", commands: "Команды", files: "Файлы", recent: "Недавние",
      synced: "Синхронизировано", syncing: "Синхронизация…", words: "слов",
      open_settings: "Настройки", graph_view: "Граф связей", toggle_theme: "Тема",
      toggle_lang: "Язык", suggestion_reason: "Общие термины и беклинки",
      compute_links: "Пересчитываю связи…", no_suggestions: "Нет предложений",
      reading_time: (m) => `${m} мин чтения`, modified: "изменён",
      ai_suggest_intro: "Возможные связи для текущей заметки:",
      graph_view: "Граф связей", graph_local: "Локальный", graph_global: "Глобальный",
      graph_depth: "Глубина", graph_loading: "Раскладка графа…", graph_fit: "Вписать",
      graph_global_warn: "Глобальный граф — на большом vault может быть тяжёлым",
      graph_filter: "Фильтр по тегам", graph_stat: (n, e) => `${n} заметок · ${e} связей`,
      graph_forces: "Силы", graph_repel: "Отталкивание", graph_linkdist: "Длина связей", graph_centerf: "Притяжение к центру", graph_group: "Группировка по тегам",
      reading_mode: "Режим чтения", exit_reading: "Выйти из чтения (Esc)",
      split_right: "Разделить вправо", close_pane: "Закрыть панель", graph_pane: "Граф рядом",
      drop_here_title: "Перетащите заметку сюда", drop_here_sub: "Перетащите вкладку из соседней панели или нажмите +",
      view_mode: "Режим", edit_mode: "Редактирование", preview_mode: "Просмотр",
    },
    en: {
      search_vault: "Search vault…",
      search_files_cmds: "Search files and commands…",
      explorer: "Files", search: "Search", tags: "Tags", starred: "Starred",
      ai_assistant: "AI assistant", chat: "Chat", suggestions: "Links", summary: "Summary",
      backlinks: "Backlinks", no_backlinks: "No backlinks",
      backlinks_count: (n) => `${n} ${n === 1 ? "backlink" : "backlinks"}`,
      mentioned_in: "Mentioned in",
      ask_anything: "Ask anything about your notes…",
      ai_empty_title: "Ask your vault",
      ai_empty_sub: "RAG over your local index. Answers cite their sources.",
      stop: "Stop", thinking: "Thinking…", sources: "Sources",
      local: "local", cloud: "cloud", cloud_answer: "Answer came from the cloud",
      indexing: (a,b) => `Indexing ${a}/${b} chunks`, indexed: "Indexed",
      llm_offline: "LLM unavailable", llm_offline_sub: "Check your server settings",
      degraded_fts: "Semantic search down — full-text only",
      reindex_banner: "Model changed — re-embedding vectors",
      recompute: "Recompute", accept: "Accept", dismiss: "Dismiss",
      new_tab: "New tab", close: "Close", split: "Split",
      no_results: "No results", commands: "Commands", files: "Files", recent: "Recent",
      synced: "Synced", syncing: "Syncing…", words: "words",
      open_settings: "Settings", graph_view: "Graph view", toggle_theme: "Theme",
      toggle_lang: "Language", suggestion_reason: "Shared terms and backlinks",
      compute_links: "Computing links…", no_suggestions: "No suggestions",
      reading_time: (m) => `${m} min read`, modified: "modified",
      ai_suggest_intro: "Possible links for the current note:",
      graph_view: "Graph view", graph_local: "Local", graph_global: "Global",
      graph_depth: "Depth", graph_loading: "Laying out graph…", graph_fit: "Fit",
      graph_global_warn: "Global graph — can be heavy on a large vault",
      graph_filter: "Filter by tag", graph_stat: (n, e) => `${n} notes · ${e} links`,
      graph_forces: "Forces", graph_repel: "Repel", graph_linkdist: "Link distance", graph_centerf: "Center pull", graph_group: "Group by tag",
      reading_mode: "Reading mode", exit_reading: "Exit reading (Esc)",
      split_right: "Split right", close_pane: "Close pane", graph_pane: "Graph beside",
      drop_here_title: "Drop a note here", drop_here_sub: "Drag a tab from the other pane, or press +",
      view_mode: "Mode", edit_mode: "Edit", preview_mode: "Preview",
    },
  };
  function plural(n, forms) {
    const m10 = n % 10, m100 = n % 100;
    if (m10 === 1 && m100 !== 11) return forms[0];
    if (m10 >= 2 && m10 <= 4 && (m100 < 12 || m100 > 14)) return forms[1];
    return forms[2];
  }
  window.NEXUS_I18N = STR;

  // ---------------- vault tree ----------------
  // type: folder|file ; files carry an id mapping into NOTES
  const VAULT = [
    { type: "folder", name: "Research", open: true, children: [
      { type: "file", id: "second-brain", name: "Second Brain.md", tag: "★" },
      { type: "file", id: "rag-pipeline", name: "RAG Pipeline.md" },
      { type: "file", id: "embeddings", name: "Embeddings.md" },
      { type: "folder", name: "Papers", open: false, children: [
        { type: "file", id: "p1", name: "Attention Is All You Need.md" },
        { type: "file", id: "p2", name: "Retrieval-Augmented Generation.md" },
        { type: "file", id: "p3", name: "HNSW Indexing.md" },
      ]},
    ]},
    { type: "folder", name: "Projects", open: true, children: [
      { type: "file", id: "nexus", name: "Nexus.md", tag: "★" },
      { type: "file", id: "local-first", name: "Local-First.md" },
      { type: "file", id: "tauri", name: "Tauri Notes.md" },
    ]},
    { type: "folder", name: "Daily", open: false, children: [
      { type: "file", id: "d1", name: "2026-06-02.md" },
      { type: "file", id: "d2", name: "2026-06-01.md" },
      { type: "file", id: "d3", name: "2026-05-31.md" },
    ]},
    { type: "file", id: "inbox", name: "Inbox.md" },
    { type: "file", id: "readme", name: "README.md" },
  ];
  window.NEXUS_VAULT = VAULT;

  // ---------------- note bodies (markdown-ish) ----------------
  const NOTES = {
    "second-brain": {
      title: "Second Brain", mtime: "2 мин / 2 min", words: 214, tags: ["pkm","method"],
      body: [
        "# Second Brain",
        "",
        "Заметки — это внешняя память. Принцип: каждая мысль связана с другими через [[wikilink]], а не сложена в папки.",
        "",
        "## Ключевые идеи",
        "- Атомарность: одна заметка — одна идея",
        "- Связи важнее иерархии — см. [[RAG Pipeline]] и [[Embeddings]]",
        "- #pkm #method",
        "",
        "> Граф связей раскрывает структуру, которую папки скрывают.",
        "",
        "Связано с проектом [[Nexus]] — локальный инструмент для такого подхода.",
      ].join("\n"),
    },
    "rag-pipeline": {
      title: "RAG Pipeline", mtime: "1 ч / 1 h", words: 388, tags: ["ai","rag"],
      body: [
        "# RAG Pipeline",
        "",
        "Retrieval-Augmented Generation: достаём релевантные чанки из [[Embeddings]] и кладём в контекст модели.",
        "",
        "## Шаги",
        "1. Чанкинг заметок (≈512 токенов с перекрытием)",
        "2. Эмбеддинг → векторный индекс ([[HNSW Indexing]])",
        "3. Поиск top-k по запросу",
        "4. Сборка контекста + промпт",
        "5. Стрим ответа со ссылками на источники",
        "",
        "Реализуется в [[Nexus]]. Базовая идея — из [[Second Brain]].",
        "",
        "#ai #rag",
      ].join("\n"),
    },
    "embeddings": {
      title: "Embeddings", mtime: "3 ч / 3 h", words: 256, tags: ["ai","math"],
      body: [
        "# Embeddings",
        "",
        "Векторное представление текста. Близость по косинусу ≈ смысловая близость.",
        "",
        "Используются в [[RAG Pipeline]] для поиска. Индексируются через [[HNSW Indexing]].",
        "",
        "## Модели",
        "- Локальные (bge, nomic) — приватность",
        "- Облачные — качество, но `☁`",
        "",
        "#ai #math",
      ].join("\n"),
    },
    "nexus": {
      title: "Nexus", mtime: "только что / now", words: 512, tags: ["project","local-first"],
      body: [
        "# Nexus",
        "",
        "Local-first редактор заметок с AI-слоем. Keyboard-first, плотный, спокойный.",
        "",
        "## Принципы",
        "- Local-first — данные у пользователя, см. [[Local-First]]",
        "- AI как слой, не модальность — [[RAG Pipeline]]",
        "- Built on [[Tauri Notes]]",
        "",
        "## Стек",
        "- React + CSS variables (токены)",
        "- CodeMirror 6 для редактора",
        "- Rust-бэкенд через Tauri commands",
        "",
        "> Тихая палитра, контент важнее хрома.",
        "",
        "#project #local-first",
      ].join("\n"),
    },
    "local-first": {
      title: "Local-First", mtime: "вчера / yesterday", words: 180, tags: ["philosophy"],
      body: [
        "# Local-First",
        "",
        "Данные живут на устройстве. Сеть — улучшение, а не зависимость.",
        "",
        "Применяется в [[Nexus]]. Конфликты решаются three-way merge.",
        "",
        "#philosophy",
      ].join("\n"),
    },
    "tauri": {
      title: "Tauri Notes", mtime: "вчера / yesterday", words: 142, tags: ["dev","rust"],
      body: [
        "# Tauri Notes",
        "",
        "Rust + WebView. Лёгкие бинарники, типизированный IPC (`invoke`, `Channel`).",
        "",
        "Используется в [[Nexus]] как оболочка.",
        "",
        "#dev #rust",
      ].join("\n"),
    },
    "inbox": { title: "Inbox", mtime: "5 мин / 5 m", words: 24, tags: [], body: "# Inbox\n\n- Прочитать [[Attention Is All You Need]]\n- Идея: связать [[Embeddings]] и заметки\n- #todo" },
    "readme": { title: "README", mtime: "1 нед / 1 w", words: 60, tags: [], body: "# README\n\nДобро пожаловать в демо-vault **Nexus**.\n\nОткрывайте заметки слева, кликайте [[wikilink]], спрашивайте AI справа.\n\nНажмите ⌘K для палитры команд." },
    "p1": { title: "Attention Is All You Need", mtime: "—", words: 90, tags: ["paper"], body: "# Attention Is All You Need\n\nTransformer-архитектура. Основа для [[Embeddings]].\n\n#paper" },
    "p2": { title: "Retrieval-Augmented Generation", mtime: "—", words: 88, tags: ["paper"], body: "# Retrieval-Augmented Generation\n\nОснова для [[RAG Pipeline]].\n\n#paper" },
    "p3": { title: "HNSW Indexing", mtime: "—", words: 76, tags: ["paper"], body: "# HNSW Indexing\n\nГраф для приближённого поиска соседей. Используется в [[Embeddings]].\n\n#paper" },
    "d1": { title: "2026-06-02", mtime: "сегодня / today", words: 40, tags: ["daily"], body: "# 2026-06-02\n\n- Доработать [[RAG Pipeline]]\n- Ревью токенов [[Nexus]]\n\n#daily" },
    "d2": { title: "2026-06-01", mtime: "—", words: 30, tags: ["daily"], body: "# 2026-06-01\n\n- Начал [[Embeddings]]\n\n#daily" },
    "d3": { title: "2026-05-31", mtime: "—", words: 22, tags: ["daily"], body: "# 2026-05-31\n\n- Идея проекта [[Nexus]]\n\n#daily" },
  };
  window.NEXUS_NOTES = NOTES;

  // title -> id resolver (for wikilinks)
  const TITLE2ID = {};
  Object.entries(NOTES).forEach(([id, n]) => { TITLE2ID[n.title.toLowerCase()] = id; });
  window.NEXUS_TITLE2ID = TITLE2ID;

  // ---------------- all tags ----------------
  window.NEXUS_TAGS = [
    { tag: "ai", count: 4 }, { tag: "rag", count: 2 }, { tag: "project", count: 1 },
    { tag: "local-first", count: 2 }, { tag: "paper", count: 3 }, { tag: "daily", count: 3 },
    { tag: "method", count: 1 }, { tag: "pkm", count: 1 }, { tag: "math", count: 1 },
    { tag: "philosophy", count: 1 }, { tag: "dev", count: 1 }, { tag: "rust", count: 1 }, { tag: "todo", count: 1 },
  ];
})();
