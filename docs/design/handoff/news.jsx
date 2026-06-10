// news.jsx — News Feed page (digest, topic clusters, cards, all states).
(function () {
  const { useState, useRef, useEffect } = React;
  const Icon = window.Icon;
  const Think = window.BrandThinking;

  const SRC = ["OpenAI", "DeepMind", "Mistral", "Hugging Face", "llama.cpp", "Simon Willison"];

  // mock feed grouped by topic
  const FEED = [
    { topic: "Модели", items: [
      { id: "n1", src: "OpenAI", url: "#", lang: "EN", t: "2 ч назад", title: "GPT-5.2 получил режим длинного контекста до 2M токенов",
        sum: "Обновление расширяет окно контекста и снижает стоимость на длинных документах; доступно в API с сегодняшнего дня.", read: false },
      { id: "n2", src: "Mistral", url: "#", lang: "EN", t: "5 ч назад", title: "Mistral Large 3 — открытые веса для коммерческого использования",
        sum: "Новая лицензия разрешает коммерцию без роялти; бенчмарки сопоставимы с закрытыми моделями среднего класса.", read: false },
      { id: "n3", src: "Hugging Face", url: "#", lang: "RU", t: "9 ч назад", title: "На HF появился рейтинг локальных моделей по скорости инференса",
        sum: "Лидерборд учитывает токены/сек на потребительских GPU и Apple Silicon.", read: true },
    ]},
    { topic: "Инференс / локальный стек", items: [
      { id: "n4", src: "llama.cpp", url: "#", lang: "EN", t: "4 ч назад", title: "llama.cpp: офлоад KV-cache на CPU без потери скорости",
        sum: "Новый аллокатор позволяет держать 70B-модели в 24 ГБ VRAM с приемлемой задержкой.", read: false },
      { id: "n5", src: "Simon Willison", url: "#", lang: "EN", t: "7 ч назад", title: "Запуск локального RAG на ноутбуке: практический разбор",
        sum: "Автор показывает пайплайн на nomic-embed + sqlite-vec без облака.", read: false },
    ]},
    { topic: "Исследования", items: [
      { id: "n6", src: "DeepMind", url: "#", lang: "EN", t: "11 ч назад", title: "Новый метод дистилляции снижает галлюцинации на 40%",
        sum: null, read: false }, // llm summary missing case (still shown)
    ]},
  ];

  const TOPICS = [["all", "Все"], ["Модели", "Модели"], ["Инференс / локальный стек", "Инференс"], ["Исследования", "Исследования"], ["Релизы", "Релизы"]];

  // condensed AI bullet summaries (generated on demand via "Сократить")
  const SUMMARIES = {
    n1: ["Окно контекста расширено до 2M токенов — целые кодовые базы и книги без нарезки.", "Длинные входы дешевле ≈на 30% благодаря новому кэшу внимания.", "Точность по фактам из середины контекста: 71% → 89%.", "В API сегодня, в ChatGPT — в течение недели."],
    n2: ["Открытые веса под Apache 2.0 — коммерция без отчислений.", "Качество на уровне закрытых моделей среднего класса.", "Варианты 8B/24B/70B на Hugging Face (safetensors, GGUF).", "Q4-версия 24B идёт на одной 16 ГБ видеокарте ≈40 ток/с."],
    n4: ["KV-cache можно офлоадить в RAM — модели 70B влезают в 24 ГБ VRAM.", "Задержка растёт умеренно (15–25%).", "Уже в main; сборки под Metal и CUDA, управляется флагом.", "Не замена большой видеокарте, но раскрывает имеющееся железо."],
    n5: ["Локальный RAG на ноутбуке без облака: nomic-embed + sqlite-vec + llama.cpp.", "Индекс по заметкам собирается за минуты, занимает десятки МБ.", "Полная приватность — данные не покидают устройство.", "Готовые скрипты и оценка стоимости в статье."],
    n3: ["Лидерборд ранжирует локальные модели по скорости (ток/с), а не только качеству.", "Замеры на потребительских GPU и Apple Silicon.", "Учтены форматы квантизации и длины контекста.", "Закрывает пробел «а как быстро это на моём железе»."],
    n6: ["Новый метод дистилляции снижает галлюцинации ≈на 40%.", "Студент перенимает «неуверенность» учителя, а не только ответы.", "Модель чаще признаёт незнание вместо выдумывания.", "Полезность ответов при этом не падает; код — после ревью."],
  };

  // full RU article bodies for the reader (LLM-translated, NOT condensed). Paragraph arrays.
  const BODIES = {
    n1: ["OpenAI выпустила обновление GPT-5.2, главным нововведением которого стал режим длинного контекста с окном до 2 миллионов токенов. Это позволяет загружать в модель целые кодовые базы, длинные юридические документы или книги без предварительной нарезки на фрагменты.",
      "По данным компании, стоимость обработки на длинных входах снижена примерно на 30% за счёт нового механизма кэширования внимания, который переиспользует уже вычисленные представления при повторных запросах к одному и тому же документу.",
      "Режим доступен в API уже сегодня; веб-интерфейс ChatGPT получит его в течение недели. Для корпоративных тарифов окно контекста включено по умолчанию, для остальных — как опция с отдельной тарификацией.",
      "Разработчики отмечают, что качество извлечения фактов из середины контекста — проблема «lost in the middle», от которой страдали предыдущие поколения, — заметно выросло. На внутреннем бенчмарке точность ответов по фактам из центральной трети документа поднялась с 71% до 89%.",
      "Независимые тесты сообщества пока ограничены, но первые замеры подтверждают: модель уверенно держит ссылки и определения, введённые в начале очень длинного ввода, и корректно использует их в конце.",
      "Критики указывают на риск чрезмерной зависимости от «грубой силы» контекстного окна вместо аккуратного retrieval: загрузка двух миллионов токенов в каждый запрос дороже и медленнее, чем точечный поиск нужных фрагментов. OpenAI отвечает, что два подхода дополняют друг друга."],
    n2: ["Mistral представила Mistral Large 3 с открытыми весами под новой лицензией, которая разрешает коммерческое использование без отчислений. Это прямой вызов закрытым моделям среднего класса и заметный сдвиг в стратегии компании.",
      "На стандартных бенчмарках — MMLU, GSM8K, HumanEval — модель показывает результаты, сопоставимые с проприетарными решениями среднего сегмента, уступая лишь флагманам вроде GPT-5 и Claude последнего поколения.",
      "Веса уже доступны на Hugging Face в форматах safetensors и GGUF. Опубликованы варианты на 8B, 24B и 70B параметров, что покрывает диапазон от ноутбука до серверной стойки.",
      "Сообщество локального инференса встретило релиз с энтузиазмом: квантизованная версия Q4 модели 24B запускается на одной потребительской видеокарте с 16 ГБ памяти и выдаёт около 40 токенов в секунду.",
      "Лицензия Apache 2.0 снимает юридические барьеры, из-за которых компании опасались строить продукты на открытых моделях. Mistral явно рассчитывает закрепиться как инфраструктурный слой для бизнеса, зарабатывая на облачном хостинге и поддержке."],
    n4: ["Команда llama.cpp добавила механизм офлоада KV-cache на системную память, который позволяет держать модели уровня 70B в пределах 24 ГБ видеопамяти без катастрофической потери скорости.",
      "Ключевая идея — новый аллокатор, который размещает «холодные» слои кэша ключей-значений в обычной оперативной памяти и подгружает их в видеопамять по мере необходимости. Раньше весь KV-cache должен был помещаться в VRAM, что жёстко ограничивало доступную длину контекста.",
      "На практике задержка вырастает умеренно — на 15–25% в зависимости от соотношения объёма контекста и пропускной способности шины PCIe. Для интерактивного использования это приемлемо, а возможность запустить большую модель там, где раньше не хватало памяти, перевешивает.",
      "Изменение уже влито в main-ветку; сборки с поддержкой Metal (Apple Silicon) и CUDA доступны в разделе релизов. Пользователям достаточно указать флаг, задающий долю слоёв для офлоада.",
      "Автор патча подчёркивает, что это не замена покупке видеокарты с большим объёмом памяти, а способ сделать локальный запуск больших моделей реалистичным на том железе, которое уже есть у людей."],
    n5: ["Саймон Уиллисон опубликовал подробный практический разбор запуска локального RAG-пайплайна на ноутбуке без единого облачного вызова — от индексации до генерации ответов.",
      "Стек получился компактным: эмбеддинги считаются моделью nomic-embed, векторное хранилище построено на расширении sqlite-vec, а генерация идёт через локальную модель, запущенную в llama.cpp. Всё работает на CPU и встроенной графике.",
      "Весь индекс по личной базе заметок собирается за несколько минут и занимает десятки мегабайт на диске. Обновление инкрементальное: при изменении заметки переиндексируется только она.",
      "Автор подчёркивает приватность подхода: данные не покидают устройство ни на одном этапе, а качество ответов на персональной базе зачастую выше, чем у универсальных облачных ассистентов, потому что контекст узкий и релевантный.",
      "В конце статьи приведены готовые скрипты и оценка стоимости: разовая настройка занимает около часа, дальнейшее использование — бесплатно."],
    n3: ["Hugging Face запустил публичный лидерборд локальных моделей, который ранжирует их по скорости инференса, измеренной в токенах в секунду, а не только по качеству ответов.",
      "Замеры проводятся на нескольких референсных конфигурациях — потребительских GPU разных классов и чипах Apple Silicon, — что даёт практический ориентир тем, кто запускает модели на своём железе.",
      "Рейтинг учитывает разные форматы квантизации (от FP16 до Q4) и длины контекста, поскольку и то, и другое сильно влияет на реальную пропускную способность.",
      "Инициатива закрывает давний пробел: большинство существующих лидербордов сравнивали только качество, оставляя вопрос «а с какой скоростью это работает на моём ноутбуке» без ответа."],
    n6: ["Исследователи DeepMind описали новый метод дистилляции знаний, который снижает частоту галлюцинаций примерно на 40% по сравнению с базовой моделью того же размера.",
      "Подход основан на обучении студенческой модели не только финальным ответам учителя, но и его «неуверенности» — полному распределению вероятностей в тех местах, где модель сомневается. Так студент перенимает не только знания, но и осознание границ этих знаний.",
      "На практике это означает, что дообученная модель чаще говорит «я не уверен» или запрашивает уточнение вместо того, чтобы уверенно выдумывать факт. Это особенно ценно для применений с высокой ценой ошибки.",
      "Авторы протестировали метод на задачах вопрос-ответ и суммаризации; снижение галлюцинаций не сопровождалось заметным падением полезности ответов.",
      "Код и чекпойнты обещают опубликовать после прохождения рецензирования; препринт уже доступен."],
  };

  function relCount(feed) { return feed.reduce((n, g) => n + g.items.filter((i) => !i.read).length, 0); }

  function Skeleton() {
    return React.createElement("div", null,
      [0,1,2,3].map((i) => React.createElement("div", { key: i, className: "news-skel" },
        React.createElement("div", { style: { flex: 1 } },
          React.createElement("div", { className: "sk-line", style: { width: (70 - i*7) + "%" } }),
          React.createElement("div", { className: "sk-line", style: { width: "92%", marginTop: 8 } }),
          React.createElement("div", { className: "sk-line", style: { width: "40%", marginTop: 8, height: 9 } })))));
  }

  function News({ t, lang, toast, offline }) {
    const [feature, setFeature] = useState(true);   // EgressFeature::NewsFeed
    const [demo, setDemo] = useState("normal");      // normal|loading|empty|error|llmfail
    const [feed, setFeed] = useState(FEED);
    const [topic, setTopic] = useState("all");
    const [unreadOnly, setUnreadOnly] = useState(false);
    const [refreshing, setRefreshing] = useState(false);
    const [showErrors, setShowErrors] = useState(false);
    const [gear, setGear] = useState(false);
    const [open, setOpen] = useState(null); // article being read (full reader)
    const [summary, setSummary] = useState(null); // null | "thinking" | string[] bullets
    const gearRef = useRef(null);

    useEffect(() => {
      if (!gear) return;
      const h = (e) => { if (gearRef.current && !gearRef.current.contains(e.target)) setGear(false); };
      window.addEventListener("mousedown", h); return () => window.removeEventListener("mousedown", h);
    }, [gear]);

    function refresh() {
      if (refreshing) return;
      setRefreshing(true);
      setTimeout(() => { setRefreshing(false); toast && toast(lang === "ru" ? "Лента обновлена" : "Feed refreshed"); }, 1800);
    }
    function markRead(gi, id) {
      setFeed((f) => f.map((g, i) => i !== gi ? g : { ...g, items: g.items.map((it) => it.id === id ? { ...it, read: !it.read } : it) }));
    }
    function toNote(it) { toast && toast((lang === "ru" ? "Создана заметка · " : "Note created · ") + it.title.slice(0, 28) + "…"); }
    function openReader(realGi, it) { setOpen(it); setSummary(null); if (!it.read) markRead(realGi, it.id); }
    function summarize() {
      if (summary === "thinking") return;
      setSummary("thinking");
      setTimeout(() => setSummary(SUMMARIES[open.id] || (open.sum ? [open.sum] : [])), 1300);
    }

    // ---- feature OFF: onboarding CTA + consent ----
    if (!feature) {
      return React.createElement("main", { className: "news" },
        React.createElement("div", { className: "news-inner" },
          React.createElement("div", { className: "news-cta" },
            React.createElement("div", { className: "cta-glyph" }, React.createElement(Icon, { name: "newspaper", size: 28 })),
            React.createElement("div", { className: "cta-title" }, lang === "ru" ? "Лента AI-новостей" : "AI news feed"),
            React.createElement("div", { className: "cta-sub" }, lang === "ru"
              ? "Раз в сутки Nexus собирает доверенные источники, отфильтровывает шум и отдаёт русские заголовки, резюме и сводку дня. Чтение без перехода по сайтам."
              : "Once a day Nexus gathers trusted sources, filters the noise, and gives you Russian headlines, summaries and a daily digest — read without leaving the app."),
            React.createElement("button", { className: "cta-btn", onClick: () => { setFeature(true); setDemo("loading"); setTimeout(() => setDemo("normal"), 2000); } },
              React.createElement(Icon, { name: "power", size: 17 }), lang === "ru" ? "Включить ленту" : "Enable feed"),
            React.createElement("div", { className: "news-consent" },
              React.createElement(Icon, { name: "shield-check", size: 16, className: "ico" }),
              React.createElement("div", null,
                React.createElement("b", null, lang === "ru" ? "Информированное согласие. " : "Informed consent. "),
                (lang === "ru" ? "Запросы будут уходить на " : "Requests will go to ") ,
                React.createElement("b", null, SRC.length + (lang === "ru" ? " доверенных источников" : " trusted sources")),
                ": ", React.createElement("span", { className: "srcs" }, SRC.join(" · ")), ".")))));
    }

    // ---- reader view: full translated article ----
    if (open) {
      const paras = BODIES[open.id] || [open.sum || ""];
      return React.createElement("main", { className: "news" },
        React.createElement("div", { className: "reader" },
          React.createElement("div", { className: "reader-bar" },
            React.createElement("button", { className: "reader-back", onClick: () => setOpen(null) },
              React.createElement(Icon, { name: "arrow-left", size: 15 }), lang === "ru" ? "К ленте" : "Back to feed"),
            React.createElement("div", { className: "reader-bar-actions" },
              React.createElement("button", { className: "reader-act" + (summary ? " on" : ""), onClick: summarize },
                React.createElement(Icon, { name: "sparkles", size: 15 }), lang === "ru" ? "Сократить" : "Summarize"),
              React.createElement("button", { className: "reader-act", onClick: () => toNote(open) },
                React.createElement(Icon, { name: "note-plus", size: 15 }), lang === "ru" ? "В заметку" : "To note"),
              React.createElement("a", { className: "reader-act", href: open.url, target: "_blank", rel: "noreferrer" },
                React.createElement(Icon, { name: "external-link", size: 15 }), lang === "ru" ? "Оригинал" : "Original"))),
          React.createElement("article", { className: "reader-doc" },
            React.createElement("div", { className: "reader-meta" },
              React.createElement("span", { className: "rm-src" }, open.src),
              React.createElement("span", null, "·"), React.createElement("span", null, open.t),
              React.createElement("span", { className: "nc-lang" }, open.lang),
              React.createElement("span", null, "·"),
              React.createElement("span", { className: "rm-trans" }, React.createElement(Icon, { name: "sparkles", size: 11 }), lang === "ru" ? "перевод AI" : "AI translation")),
            React.createElement("h1", { className: "reader-title" }, open.title),
            summary === "thinking" ? React.createElement("div", { className: "reader-summary thinking" },
              React.createElement(Think, { size: 18 }), React.createElement("span", { className: "mt-label" }, lang === "ru" ? "Сокращаю…" : "Summarizing…")) : null,
            (summary && summary !== "thinking") ? React.createElement("div", { className: "reader-summary" },
              React.createElement("div", { className: "rs-head" },
                React.createElement(Icon, { name: "sparkles", size: 14 }), lang === "ru" ? "Кратко" : "TL;DR",
                React.createElement("button", { className: "rs-close", onClick: () => setSummary(null), title: lang === "ru" ? "Скрыть" : "Hide" }, React.createElement(Icon, { name: "x", size: 13 }))),
              React.createElement("ul", { className: "rs-list" },
                summary.map((s, i) => React.createElement("li", { key: i }, s)))) : null,
            open.sum ? React.createElement("div", { className: "reader-lede" }, open.sum) : null,
            paras.map((p, i) => React.createElement("p", { key: i, className: "reader-p" }, p)),
            React.createElement("div", { className: "reader-foot" },
              React.createElement(Icon, { name: "info", size: 13 }),
              lang === "ru" ? "Полный текст переведён локальной моделью. Оригинал — по ссылке." : "Full text translated by a local model. Original at the link."))));
    }

    const offlineNow = offline;
    const filtered = feed
      .filter((g) => topic === "all" || g.topic === topic)
      .map((g) => ({ ...g, items: g.items.filter((it) => !unreadOnly || !it.read) }))
      .filter((g) => g.items.length);

    function gearMenu() {
      return React.createElement("div", { className: "ai-menu", style: { minWidth: 230 }, ref: gearRef },
        React.createElement("div", { className: "ai-menu-head" }, lang === "ru" ? "Страница новостей" : "News page"),
        React.createElement("button", { className: "ai-menu-item", onClick: () => { setGear(false); setFeature(false); } },
          React.createElement(Icon, { name: "power", size: 15, className: "ico" }), lang === "ru" ? "Выключить ленту" : "Disable feed"),
        React.createElement("div", { className: "ai-menu-head", style: { marginTop: 4 } }, lang === "ru" ? "Состояние (демо)" : "State (demo)"),
        [["normal", lang === "ru" ? "Лента" : "Feed"], ["loading", lang === "ru" ? "Первый прогон" : "First run"], ["empty", lang === "ru" ? "Пустой день" : "Empty day"], ["llmfail", lang === "ru" ? "LLM упал" : "LLM failed"], ["error", lang === "ru" ? "Ошибка прогона" : "Run error"]].map(([v, label]) =>
          React.createElement("button", { key: v, className: "ai-menu-item" + (demo === v ? " on" : ""), onClick: () => { setDemo(v); setGear(false); },
            style: demo === v ? { color: "var(--color-accent)" } : null },
            React.createElement(Icon, { name: demo === v ? "check" : "dot", size: 14, className: "ico" }), label)));
    }

    const header = React.createElement("div", { className: "news-digest ai" },
      React.createElement("div", { className: "nd-head" },
        React.createElement("div", { className: "nd-title" }, React.createElement(Icon, { name: "sparkles", size: 16 }),
          lang === "ru" ? "Сводка дня" : "Daily digest", React.createElement("span", { className: "nd-badge" }, "AI")),
        React.createElement("button", { className: "nd-refresh" + (refreshing ? " spinning" : ""), onClick: refresh },
          React.createElement(Icon, { name: "refresh", size: 14 }), refreshing ? (lang === "ru" ? "Собираю…" : "Fetching…") : (lang === "ru" ? "Обновить" : "Refresh"))),
      React.createElement("div", { className: "nd-body" }, lang === "ru"
        ? React.createElement(React.Fragment, null, "Главное за сутки: ", React.createElement("strong", null, "GPT-5.2"), " расширил контекст до 2M токенов, ", React.createElement("strong", null, "Mistral Large 3"), " вышел с открытыми весами под коммерцию. В локальном стеке — офлоад KV-cache в llama.cpp снимает барьер 70B на 24 ГБ. В исследованиях — новый метод дистилляции против галлюцинаций.")
        : "Today's highlights: GPT-5.2 extended context to 2M tokens, Mistral Large 3 shipped with open commercial weights, llama.cpp's KV-cache offload removes the 70B-on-24GB barrier, and a new distillation method cuts hallucinations."),
      React.createElement("div", { className: "nd-meta" },
        React.createElement("span", null, lang === "ru" ? "Обновлено 12 мин назад" : "Updated 12 min ago"),
        React.createElement("span", null, "·"),
        React.createElement("span", null, lang === "ru" ? "6 статей из 6 источников" : "6 items from 6 sources"),
        React.createElement("span", null, "·"),
        React.createElement("span", { className: "nd-warn", onClick: () => setShowErrors((v) => !v) },
          React.createElement(Icon, { name: "alert", size: 12 }), lang === "ru" ? "5 из 6 источников" : "5 of 6 sources")),
      showErrors ? React.createElement("div", { className: "nd-errors" },
        React.createElement("div", { className: "er" }, React.createElement(Icon, { name: "x", size: 13 }),
          lang === "ru" ? "Mistral блог — таймаут (15с), будет повтор в следующем прогоне" : "Mistral blog — timeout (15s), will retry next run")) : null);

    let bodyEl;
    if (demo === "loading") bodyEl = React.createElement("div", null,
      React.createElement("div", { className: "news-rubric", style: { display: "flex", alignItems: "center", gap: 9 } }, React.createElement(Think, { size: 16 }), React.createElement("span", { className: "mt-label" }, lang === "ru" ? "Собираю новости…" : "Gathering news…")),
      React.createElement(Skeleton));
    else if (demo === "empty") bodyEl = React.createElement("div", { className: "news-state" },
      React.createElement("div", { className: "ns-glyph" }, React.createElement(Icon, { name: "newspaper", size: 24 })),
      React.createElement("div", { className: "ns-title" }, lang === "ru" ? "Свежих новостей нет" : "No fresh news"),
      React.createElement("div", { className: "ns-sub" }, lang === "ru" ? "Следующий автопрогон — в 09:00. Можно обновить вручную." : "Next auto-run at 09:00. You can refresh manually."),
      React.createElement("button", { className: "cta-btn", style: { height: 38 }, onClick: refresh }, React.createElement(Icon, { name: "refresh", size: 15 }), lang === "ru" ? "Обновить" : "Refresh"));
    else if (demo === "error") bodyEl = React.createElement("div", { className: "news-state" },
      React.createElement("div", { className: "ns-glyph", style: { background: "var(--color-danger-soft)", color: "var(--color-danger)" } }, React.createElement(Icon, { name: "alert", size: 24 })),
      React.createElement("div", { className: "ns-title" }, lang === "ru" ? "Не удалось собрать ленту" : "Couldn't fetch the feed"),
      React.createElement("div", { className: "ns-sub" }, lang === "ru" ? "Прошлые данные сохранены. Проверьте сеть и повторите." : "Previous data is kept. Check your connection and retry."),
      React.createElement("button", { className: "cta-btn", style: { height: 38 }, onClick: refresh }, React.createElement(Icon, { name: "refresh", size: 15 }), lang === "ru" ? "Повторить" : "Retry"));
    else bodyEl = filtered.map((g, gi) => React.createElement("div", { key: g.topic },
      React.createElement("div", { className: "news-rubric" }, g.topic, React.createElement("span", { className: "rc" }, g.items.length)),
      React.createElement("div", { className: "news-list" },
        g.items.map((it) => {
          const realGi = feed.findIndex((x) => x.topic === g.topic);
          const llmMissing = it.sum == null;
          return React.createElement("div", { key: it.id, className: "news-card" + (it.read ? " read" : "") },
            React.createElement("div", { className: "nc-unread" }, React.createElement("i", null)),
            React.createElement("div", { className: "nc-main" },
              React.createElement("span", { className: "nc-title", onClick: () => openReader(realGi, it) },
                (demo === "llmfail" && llmMissing) ? it.title : it.title,
                React.createElement(Icon, { name: "chevron" })),
              (demo === "llmfail" || llmMissing)
                ? React.createElement("div", { className: "nc-summary missing" }, lang === "ru" ? "Резюме недоступно — показан оригинальный заголовок" : "Summary unavailable — original headline shown")
                : React.createElement("div", { className: "nc-summary" }, it.sum),
              React.createElement("div", { className: "nc-meta" },
                React.createElement("span", { className: "nc-src" }, it.src),
                React.createElement("span", null, "·"), React.createElement("span", null, it.t),
                React.createElement("span", { className: "nc-lang" }, it.lang),
                React.createElement("div", { className: "nc-actions" },
                  React.createElement("button", { className: "nc-act" + (it.read ? " on" : ""), title: lang === "ru" ? "Прочитано" : "Read", onClick: () => markRead(realGi, it.id) },
                    React.createElement(Icon, { name: it.read ? "eye-off" : "eye", size: 15 })),
                  React.createElement("button", { className: "nc-act", title: lang === "ru" ? "В заметку" : "To note", onClick: () => toNote(it) },
                    React.createElement(Icon, { name: "note-plus", size: 15 }))))));
        }))));

    return React.createElement("main", { className: "news" },
      React.createElement("div", { className: "news-inner" },
        offlineNow ? React.createElement("div", { className: "news-offline-banner" },
          React.createElement(Icon, { name: "wifi-off", size: 15 }),
          lang === "ru" ? "Офлайн — лента на паузе. Показаны данные последнего прогона." : "Offline — feed paused. Showing last run's data.") : null,
        React.createElement("div", { style: { display: "flex", alignItems: "flex-start", gap: 10 } },
          React.createElement("div", { style: { flex: 1, minWidth: 0 } }, header),
          React.createElement("div", { style: { position: "relative" }, ref: gearRef },
            React.createElement("button", { className: "tb-btn", style: { marginTop: 4 }, onClick: () => setGear((v) => !v), title: lang === "ru" ? "Настройки страницы" : "Page settings" },
              React.createElement(Icon, { name: "settings", size: 16 })),
            gear ? gearMenu() : null)),
        (demo === "normal") ? React.createElement("div", { className: "news-filters" },
          TOPICS.map(([v, label]) => React.createElement("button", { key: v, className: "nf-chip" + (topic === v ? " on" : ""), onClick: () => setTopic(v) },
            label, v === "all" ? React.createElement("span", { className: "cnt" }, relCount(feed)) : null)),
          React.createElement("label", { className: "nf-toggle" },
            React.createElement("div", { className: "set-switch" + (unreadOnly ? " on" : ""), role: "switch", "aria-checked": unreadOnly, tabIndex: 0, onClick: () => setUnreadOnly((v) => !v) }, React.createElement("i", null)),
            lang === "ru" ? "Непрочитанные" : "Unread")) : null,
        bodyEl,
        (demo === "normal" && filtered.length === 0) ? React.createElement("div", { className: "news-state" },
          React.createElement("div", { className: "ns-glyph" }, React.createElement(Icon, { name: "check", size: 24 })),
          React.createElement("div", { className: "ns-title" }, lang === "ru" ? "Всё прочитано" : "All caught up"),
          React.createElement("div", { className: "ns-sub" }, lang === "ru" ? "В этой теме нет непрочитанных." : "No unread items in this topic.")) : null));
  }
  window.News = News;
})();
