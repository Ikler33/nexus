// home.jsx — Home dashboard as an in-app React view (ported from the static page).
(function () {
  const { useEffect, useRef } = React;

  // approved dashboard markup (RU). Injected as-is; boot + clicks wired in useEffect.
  const INNER = `
        <div class="home-header">
          <svg class="header-constellation" viewBox="0 0 168 132" fill="none" aria-hidden="true">
            <g stroke="currentColor" stroke-width="1.4" stroke-linecap="round">
              <line x1="40" y1="38" x2="92" y2="22"/><line x1="92" y1="22" x2="128" y2="58"/><line x1="40" y1="38" x2="66" y2="86"/><line x1="66" y1="86" x2="128" y2="58"/><line x1="128" y1="58" x2="150" y2="100"/>
            </g>
            <g fill="currentColor">
              <circle cx="40" cy="38" r="4"/><circle cx="92" cy="22" r="3"/><circle cx="128" cy="58" r="5"/><circle cx="66" cy="86" r="3"/><circle cx="150" cy="100" r="2.5"/>
            </g>
          </svg>
          <div class="home-greeting-wrap">
            <div class="home-greeting" data-i="greeting">Добрый день, <em>Артём</em></div>
            <div class="home-sub" data-i="sub">среда, 8 июня · 847 заметок · 12 изменений сегодня</div>
            <div class="home-meta">
              <span class="h-chip live"><span class="dot"></span>ollama · qwen3:35b</span>
              <span class="h-chip"><svg class="ico" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="22" y1="12" x2="2" y2="12"/><path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z"/></svg>vault: ~/notes</span>
              <span class="h-chip" data-i="streakchip"><svg class="ico" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M11.5 2.5 14 8l5.5.5-4 4 1 5.5-5-3-5 3 1-5.5-4-4L9 8z"/></svg>серия: 23 дня</span>
            </div>
          </div>
        </div>

        <div class="home-search">
          <svg class="ico" width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="7"/><path d="m21 21-4.3-4.3"/></svg>
          <span class="hs-text" data-i="search">Поиск по базе или вопрос к ассистенту…</span>
          <span class="kbd">/</span><span class="kbd">?</span>
        </div>

        <div class="continue">
          <div>
            <div class="c-eyebrow"><svg class="ico" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m5 12 7-7 7 7"/><path d="M12 19V5"/></svg><span data-i="continue">Продолжить</span></div>
            <div class="c-title">RAG Pipeline</div>
            <div class="c-snippet" data-i="csnip">Retrieval-Augmented Generation: достаём релевантные чанки из эмбеддингов и кладём в контекст модели. Чанкинг, эмбеддинг, поиск top-k, сборка контекста и стриминг ответа со ссылками…</div>
            <div class="c-meta" data-i="cmeta"><span>Research</span><span>·</span><span>редактировалась 1 ч назад</span><span>·</span><span>388 слов</span></div>
          </div>
          <button class="c-go"><span data-i="open">Открыть</span> <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12h14"/><path d="m12 5 7 7-7 7"/></svg></button>
        </div>

        <div class="quick-actions">
          <button class="qa" data-act="new"><svg class="ico" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12h14"/><path d="M12 5v14"/></svg><span data-i="qa_new">Новая заметка</span></button>
          <button class="qa" data-act="daily"><svg class="ico" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><polyline points="12 7 12 12 15 14"/></svg><span data-i="qa_daily">Daily note</span></button>
          <button class="qa" data-act="thought"><svg class="ico" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z"/><path d="m15 5 4 4"/></svg><span data-i="qa_thought">Быстрая мысль</span></button>
          <button class="qa" data-act="graph"><svg class="ico" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="2"/><circle cx="5" cy="6" r="2"/><circle cx="19" cy="7" r="2"/><circle cx="18" cy="18" r="2"/><path d="m7 7 3 3"/><path d="m17 8-3.5 3"/><path d="m13.5 13.5 3 3.5"/></svg><span data-i="qa_graph">Граф</span></button>
          <button class="qa" data-act="reindex"><svg class="ico" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/><path d="M8 16H3v5"/></svg><span data-i="qa_reindex">Переиндексировать</span></button>
        </div>

        <div class="sec-label" data-i="s_summary">сводка</div>
        <div class="grid-2">
          <div class="h-card ai">
            <div class="card-head">
              <div class="card-title"><svg class="ico" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M9.94 14.66 9 17l-.94-2.34a4 4 0 0 0-2.72-2.72L3 11l2.34-.94a4 4 0 0 0 2.72-2.72L9 5l.94 2.34a4 4 0 0 0 2.72 2.72L15 11l-2.34.94a4 4 0 0 0-2.72 2.72Z"/></svg><span data-i="c_brief">Сводка дня</span><span class="ai-badge">AI</span></div>
              <button class="card-act"><svg class="ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/><path d="M8 16H3v5"/></svg><span data-i="refresh">обновить</span></button>
            </div>
            <p class="brief-text" data-i="brief">Активная работа над <strong>архитектурой агентов</strong> — 3 новые заметки по KV-cache и офлоаду слоёв. Заметка <strong>«payment-service анализ»</strong> не открывалась 32 дня. Прогресс по GPU-сетапу близок к завершению.</p>
            <div class="brief-tags"><span class="brief-tag">#llm-агенты</span><span class="brief-tag">#inference</span><span class="brief-tag">#go</span></div>
          </div>

          <div class="h-card">
            <div class="card-head">
              <div class="card-title"><svg class="ico" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><polyline points="12 7 12 12 15 14"/></svg><span data-i="c_recent">Недавние</span></div>
              <button class="card-act" data-i="all">все →</button>
            </div>
            <div class="h-list">
              <div class="h-row" data-note="rag-pipeline"><svg class="ico-f" width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><path d="M14 2v6h6"/></svg><div class="r-body"><div class="r-name">RAG Pipeline</div><div class="r-meta">Research</div></div><span class="r-time">12 мин</span></div>
              <div class="h-row" data-note="nexus"><svg class="ico-f" width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><path d="M14 2v6h6"/></svg><div class="r-body"><div class="r-name">Nexus</div><div class="r-meta">Projects</div></div><span class="r-time">1 ч</span></div>
              <div class="h-row" data-note="embeddings"><svg class="ico-f" width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><path d="M14 2v6h6"/></svg><div class="r-body"><div class="r-name">Embeddings</div><div class="r-meta">Research</div></div><span class="r-time">3 ч</span></div>
              <div class="h-row" data-note="local-first"><svg class="ico-f" width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><path d="M14 2v6h6"/></svg><div class="r-body"><div class="r-name">Local-First</div><div class="r-meta">Projects</div></div><span class="r-time">вчера</span></div>
              <div class="h-row" data-note="second-brain"><svg class="ico-f" width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><path d="M14 2v6h6"/></svg><div class="r-body"><div class="r-name">Second Brain</div><div class="r-meta">Research</div></div><span class="r-time">вчера</span></div>
            </div>
          </div>
        </div>

        <div class="sec-label" data-i="s_activity">активность</div>
        <div class="grid-2">
          <div class="h-card">
            <div class="card-head">
              <div class="card-title"><svg class="ico" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><rect width="18" height="18" x="3" y="4" rx="2"/><path d="M3 10h18"/><path d="M8 2v4"/><path d="M16 2v4"/></svg><span data-i="c_activity">Активность</span></div>
              <span class="heat-streak" id="streak"></span>
            </div>
            <div class="act-metrics">
              <div class="act-metric">
                <div class="am-top"><span class="am-val">18</span><span class="am-trend up"><svg class="ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="m5 12 7-7 7 7"/><path d="M12 19V5"/></svg>12%</span></div>
                <div class="am-label" data-i="m_notes">заметок за неделю</div>
              </div>
              <div class="act-metric">
                <div class="am-top"><span class="am-val">4.2k</span><span class="am-trend up"><svg class="ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="m5 12 7-7 7 7"/><path d="M12 19V5"/></svg>23%</span></div>
                <div class="am-label" data-i="m_words">слов написано</div>
              </div>
              <div class="act-metric">
                <div class="am-top"><span class="am-val">47</span><span class="am-trend up"><svg class="ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="m5 12 7-7 7 7"/><path d="M12 19V5"/></svg>8%</span></div>
                <div class="am-label" data-i="m_links">новых связей</div>
              </div>
            </div>
            <div class="heat-wrap">
              <div class="heat-grid" id="heat"></div>
              <div class="heat-legend">
                <span data-i="less">меньше</span>
                <span class="scale"><i style="background:var(--color-surface)"></i><i class="l1" style="background:color-mix(in oklch, var(--color-accent) 28%, var(--color-surface))"></i><i class="l2" style="background:color-mix(in oklch, var(--color-accent) 52%, var(--color-surface))"></i><i class="l3" style="background:color-mix(in oklch, var(--color-accent) 76%, var(--color-surface))"></i><i style="background:var(--color-accent)"></i></span>
                <span data-i="more">больше · последние 17 недель</span>
              </div>
            </div>
            <div class="act-goal">
              <span class="ag-ic"><svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 2c1 3 3 4.5 3 7a3 3 0 0 1-6 0c0-1 .4-1.8 1-2.5C9 9 7 10.5 7 14a5 5 0 0 0 10 0c0-4.5-3-8-5-12Z"/></svg></span>
              <span class="ag-text" data-i="goal">Лучшая серия — <b>31 день</b>. До личного рекорда осталось <b>8 дней</b>.</span>
              <span class="ag-bar"><i></i></span>
            </div>
          </div>

          <div class="h-card graph-card">
            <div class="card-head">
              <div class="card-title"><svg class="ico" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="2"/><circle cx="5" cy="6" r="2"/><circle cx="19" cy="7" r="2"/><circle cx="18" cy="18" r="2"/><path d="m7 7 3 3"/><path d="m17 8-3.5 3"/><path d="m13.5 13.5 3 3.5"/></svg><span data-i="c_graph">Граф связей</span></div>
              <button class="card-act" data-i="gopen">открыть →</button>
            </div>
            <div class="graph-mini" id="gmini">
              <svg id="gmini-svg" viewBox="0 0 400 360" preserveAspectRatio="xMidYMid meet" aria-hidden="true"></svg>
              <span class="gm-cta" data-i="gcta">847 заметок · 1.9k связей →</span>
            </div>
          </div>
        </div>

        <div class="sec-label" data-i="s_projects">проекты</div>
        <div class="grid-2">
          <div class="h-card">
            <div class="card-head">
              <div class="card-title"><svg class="ico" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M12 3a15 15 0 0 1 0 18 15 15 0 0 1 0-18"/><path d="M3 12h18"/></svg><span data-i="c_goals">Прогресс целей</span></div>
              <button class="card-act" data-i="allgoals">все цели →</button>
            </div>
            <div class="prog-list">
              <div><div class="prog-row"><span class="prog-name">Агент анализа кода</span><span class="prog-pct">65%</span></div><div class="prog-track"><div class="prog-fill c-accent" style="--target:65%"></div></div></div>
              <div><div class="prog-row"><span class="prog-name">YouTube-система</span><span class="prog-pct">30%</span></div><div class="prog-track"><div class="prog-fill c-ai" style="--target:30%"></div></div></div>
              <div><div class="prog-row"><span class="prog-name">GPU inference сетап</span><span class="prog-pct">80%</span></div><div class="prog-track"><div class="prog-fill c-success" style="--target:80%"></div></div></div>
            </div>
          </div>

          <div class="h-card">
            <div class="card-head">
              <div class="card-title"><svg class="ico" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><line x1="4" y1="9" x2="20" y2="9"/><line x1="4" y1="15" x2="20" y2="15"/><line x1="10" y1="3" x2="8" y2="21"/><line x1="16" y1="3" x2="14" y2="21"/></svg><span data-i="c_stats">Статистика</span></div>
            </div>
            <div class="stat-grid">
              <div class="stat"><div class="stat-val">847</div><div class="stat-label" data-i="st_notes">заметок</div></div>
              <div class="stat"><div class="stat-val">12</div><div class="stat-label" data-i="st_changes">изменений сегодня</div></div>
              <div class="stat"><div class="stat-val">94</div><div class="stat-label" data-i="st_orphan">без обратных ссылок</div></div>
              <div class="stat"><div class="stat-val">23</div><div class="stat-label" data-i="st_streak">дня серия</div></div>
            </div>
          </div>
        </div>

        <div class="sec-label" data-i="s_attention">требует внимания</div>
        <div class="grid-2">
          <div class="h-card">
            <div class="card-head">
              <div class="card-title"><svg class="ico" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/></svg><span data-i="c_stale">Stale radar</span></div>
              <button class="card-act" data-i="all2">все →</button>
            </div>
            <div class="h-list">
              <div class="stale-row" data-note="local-first"><span class="stale-dot hot"></span><span class="stale-name">payment-service анализ</span><span class="stale-days">32 дня</span><span class="stale-do" data-i="do_upd">обновить</span></div>
              <div class="stale-row" data-note="embeddings"><span class="stale-dot hot"></span><span class="stale-name">CMP 50HX характеристики</span><span class="stale-days">28 дней</span><span class="stale-do" data-i="do_arch">архив</span></div>
              <div class="stale-row" data-note="nexus"><span class="stale-dot warm"></span><span class="stale-name">Идеи YouTube-сценариев</span><span class="stale-days">15 дней</span><span class="stale-do" data-i="do_split">разбить</span></div>
              <div class="stale-row" data-note="second-brain"><span class="stale-dot warm"></span><span class="stale-name">Ролевое разделение агентов</span><span class="stale-days">14 дней</span><span class="stale-do" data-i="do_upd2">обновить</span></div>
            </div>
          </div>

          <div class="h-card ai">
            <div class="card-head">
              <div class="card-title"><svg class="ico" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M9.1 9a3 3 0 0 1 5.8 1c0 2-3 3-3 3"/><path d="M12 17h.01"/></svg><span data-i="c_oq">Открытые вопросы</span><span class="ai-badge">AI</span></div>
              <button class="card-act"><svg class="ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/><path d="M8 16H3v5"/></svg><span data-i="refresh2">обновить</span></button>
            </div>
            <div class="oq-list">
              <div class="oq">Как ограничить KV-cache при большом контексте?</div>
              <div class="oq">Какую роль дать reviewer-агенту в пайплайне?</div>
              <div class="oq">Стоит ли использовать Gemma 4 для stylist-роли?</div>
            </div>
          </div>
        </div>

        <div class="sec-label" data-i="s_analysis">анализ</div>
        <div class="grid-full">
          <div class="h-card ai">
            <div class="card-head">
              <div class="card-title"><svg class="ico" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="m16 3 4 4-4 4"/><path d="M20 7H4"/><path d="m8 21-4-4 4-4"/><path d="M4 17h16"/></svg><span data-i="c_drift">Смещение фокуса</span><span class="ai-badge">AI</span></div>
              <button class="card-act"><svg class="ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/><path d="M8 16H3v5"/></svg><span data-i="refresh3">обновить</span></button>
            </div>
            <p class="drift-text" data-i="drift">Последние 3 дня фокус смещён на <em>оптимизацию inference</em> и детали железа, тогда как долгосрочные цели акцентируют <strong>архитектуру агентов</strong> и <strong>YouTube-систему</strong>. Возможно, стоит вернуть фокус к высокоуровневому проектированию.</p>
          </div>
        </div>
  `;

  // EN overlay — chrome strings only (vault content stays RU)
  const EN = {
    greeting: "Good afternoon, <em>Artem</em>",
    sub: "Wednesday, June 8 · 847 notes · 12 changes today",
    streakchip: '<svg class="ico" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M11.5 2.5 14 8l5.5.5-4 4 1 5.5-5-3-5 3 1-5.5-4-4L9 8z"/></svg>streak: 23 days',
    search: "Search your vault or ask the assistant…",
    continue: "Continue", open: "Open",
    csnip: "Retrieval-Augmented Generation: pull relevant chunks from the embeddings and feed the model's context. Chunking, embedding, top-k retrieval, context assembly, and a streamed answer with citations…",
    cmeta: "<span>Research</span><span>·</span><span>edited 1 h ago</span><span>·</span><span>388 words</span>",
    qa_new: "New note", qa_daily: "Daily note", qa_thought: "Quick thought", qa_graph: "Graph", qa_reindex: "Reindex",
    s_summary: "summary", s_activity: "activity", s_projects: "projects", s_attention: "needs attention", s_analysis: "analysis",
    c_brief: "Daily brief", c_recent: "Recent", c_activity: "Activity", c_graph: "Graph", c_goals: "Goals progress", c_stats: "Stats", c_stale: "Stale radar", c_oq: "Open questions", c_drift: "Focus drift",
    refresh: "refresh", refresh2: "refresh", refresh3: "refresh", all: "all →", all2: "all →", allgoals: "all goals →", gopen: "open →",
    brief: "Active work on <strong>agent architecture</strong> — 3 new notes on KV-cache and layer offload. <strong>“payment-service analysis”</strong> hasn't been opened in 32 days. GPU setup is close to done.",
    m_notes: "notes this week", m_words: "words written", m_links: "new links",
    less: "less", more: "more · last 17 weeks",
    goal: "Best streak — <b>31 days</b>. <b>8 days</b> to your personal record.",
    gcta: "847 notes · 1.9k links →",
    st_notes: "notes", st_changes: "changes today", st_orphan: "no backlinks", st_streak: "day streak",
    do_upd: "update", do_arch: "archive", do_split: "split", do_upd2: "update",
    drift: "Over the last 3 days focus shifted to <em>inference optimization</em> and hardware details, while long-term goals emphasize <strong>agent architecture</strong> and the <strong>YouTube system</strong>. Consider returning to high-level design.",
  };

  function Home({ t, lang, onOpenNote, onNewNote, onGraph, onSearch, toast }) {
    const rootRef = useRef(null);

    useEffect(() => {
      const root = rootRef.current; if (!root) return;
      const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
      const wait = (ms) => new Promise((r) => setTimeout(r, ms));
      const timers = [];
      const after = (fn, ms) => { const id = setTimeout(fn, ms); timers.push(id); return id; };

      // EN overlay
      if (lang === "en") {
        root.querySelectorAll("[data-i]").forEach((el) => {
          const k = el.getAttribute("data-i");
          if (EN[k] != null) el.innerHTML = EN[k];
        });
      }

      function countUp(el, dur = 900) {
        const raw = el.getAttribute("data-val") || el.textContent.trim();
        el.setAttribute("data-val", raw);
        const m = raw.match(/^([\d.]+)(.*)$/); if (!m) return;
        const target = parseFloat(m[1]), suffix = m[2], decimals = (m[1].split(".")[1] || "").length;
        if (reduce) { el.textContent = raw; return; }
        const t0 = performance.now();
        const tick = (now) => {
          const p = Math.min(1, (now - t0) / dur), e = 1 - Math.pow(1 - p, 3);
          el.textContent = (target * e).toFixed(decimals) + suffix;
          if (p < 1) requestAnimationFrame(tick); else el.textContent = raw;
        };
        requestAnimationFrame(tick);
      }
      function thinkingMark() {
        return '<svg class="bt-mark" viewBox="0 0 32 32" fill="none" aria-hidden="true">'
          + '<g><line class="bt-edge" x1="16" y1="16" x2="8" y2="8"/><line class="bt-edge" x1="16" y1="16" x2="25" y2="11"/><line class="bt-edge" x1="16" y1="16" x2="12" y2="25"/><line class="bt-edge" x1="25" y1="11" x2="12" y2="25"/></g>'
          + '<g><circle class="bt-node" cx="16" cy="16" r="3.4"/><circle class="bt-node n2" cx="8" cy="8" r="2.5"/><circle class="bt-node n3" cx="25" cy="11" r="2.5"/><circle class="bt-node n4" cx="12" cy="25" r="2.5"/></g></svg>';
      }
      function prepAICard(card, label) {
        const head = card.querySelector(".card-head");
        const content = document.createElement("div");
        content.className = "ai-content pending";
        [...card.children].forEach((c) => { if (c !== head) content.appendChild(c); });
        card.appendChild(content);
        const think = document.createElement("div");
        think.className = "ai-think";
        think.innerHTML = thinkingMark() + '<span class="bt-label">' + label + '</span>';
        card.appendChild(think);
        return () => { think.classList.add("gone"); content.classList.remove("pending"); after(() => think.remove(), 500); };
      }
      function buildHeat() {
        const heat = root.querySelector("#heat"); const WEEKS = 17, DAYS = 7, total = WEEKS * DAYS, els = [];
        for (let i = 0; i < total; i++) {
          const recency = i / total, r = Math.random(), fromEnd = total - 1 - i;
          let level;
          if (fromEnd < 23) level = r < 0.32 ? 2 : (r < 0.72 ? 3 : 4);
          else { const w = r * (0.45 + recency * 0.75); level = w < 0.34 ? 0 : w < 0.55 ? 1 : w < 0.78 ? 2 : w < 0.93 ? 3 : 4; }
          const d = document.createElement("div");
          d.className = "heat-cell seed" + (level ? " l" + level : "");
          heat.appendChild(d); els.push(d);
        }
        return els;
      }
      function revealHeat(els) {
        const s = root.querySelector("#streak");
        if (s && !s.innerHTML) s.innerHTML = '<span id="streak-num">23</span> <small>' + (lang === "en" ? "days in a row" : "дня подряд") + '</small>';
        if (reduce) { els.forEach((d) => d.classList.replace("seed", "pop")); return; }
        els.forEach((d, i) => { const col = Math.floor(i / 7); after(() => d.classList.replace("seed", "pop"), col * 42 + (i % 7) * 6); });
      }
      function buildGraph() {
        const svg = root.querySelector("#gmini-svg"); if (!svg) return { edgeEls: [], nodeEls: [] };
        const NS = "http://www.w3.org/2000/svg", W = 400, H = 360;
        let seed = 7; const rnd = () => { seed = (seed * 1103515245 + 12345) & 0x7fffffff; return seed / 0x7fffffff; };
        const hubs = [[200,175,"hub",7,8.5],[120,95,"link",5,6],[300,110,"ai",5,6.5],[110,260,"tag",4,5.5],[305,255,"",5,6]];
        const nodes = [], edges = [];
        hubs.forEach((h) => {
          const [hx,hy,cls,sats,r] = h, idx = nodes.length;
          nodes.push({ x: hx, y: hy, r, cls });
          for (let s = 0; s < sats; s++) {
            const ang = (s/sats)*Math.PI*2 + rnd()*1.1, dist = 34 + rnd()*30;
            let x = Math.max(16, Math.min(W-16, hx+Math.cos(ang)*dist)), y = Math.max(16, Math.min(H-16, hy+Math.sin(ang)*dist));
            const si = nodes.length;
            nodes.push({ x, y, r: 2.4 + rnd()*2.2, cls: rnd() > 0.82 ? cls : "" });
            edges.push({ a: idx, b: si });
            if (s > 0 && rnd() > 0.7) edges.push({ a: si, b: si-1 });
          }
        });
        const hubNodeIdx = []; let cursor = 0;
        hubs.forEach((h) => { hubNodeIdx.push(cursor); cursor += 1 + h[3]; });
        for (let i = 1; i < hubNodeIdx.length; i++) edges.push({ a: hubNodeIdx[0], b: hubNodeIdx[i], hot: true });
        edges.push({ a: hubNodeIdx[1], b: hubNodeIdx[3] }); edges.push({ a: hubNodeIdx[2], b: hubNodeIdx[4] });
        const floatG = document.createElementNS(NS, "g"); floatG.setAttribute("class", "gm-float");
        const gE = document.createElementNS(NS, "g"); const edgeEls = [];
        edges.forEach((e) => {
          const A = nodes[e.a], B = nodes[e.b]; if (!A || !B) return;
          const ln = document.createElementNS(NS, "line");
          ln.setAttribute("x1", A.x); ln.setAttribute("y1", A.y); ln.setAttribute("x2", B.x); ln.setAttribute("y2", B.y);
          const len = Math.hypot(B.x-A.x, B.y-A.y);
          ln.setAttribute("class", "gm-edge" + (e.hot ? " hot" : "")); ln.style.setProperty("--len", len.toFixed(1));
          gE.appendChild(ln); edgeEls.push(ln);
        });
        const gN = document.createElementNS(NS, "g"); const nodeEls = [];
        nodes.forEach((n) => {
          const c = document.createElementNS(NS, "circle");
          c.setAttribute("cx", n.x); c.setAttribute("cy", n.y); c.setAttribute("r", n.r);
          c.setAttribute("class", "gm-node seed" + (n.cls ? " " + n.cls : ""));
          gN.appendChild(c); nodeEls.push(c);
        });
        floatG.appendChild(gE); floatG.appendChild(gN); svg.appendChild(floatG);
        return { edgeEls, nodeEls };
      }
      function revealGraph(g) {
        const mini = root.querySelector("#gmini");
        if (reduce) { g.edgeEls.forEach((l) => l.classList.add("draw")); g.nodeEls.forEach((n) => n.classList.replace("seed", "pop")); mini.classList.add("float"); return; }
        g.edgeEls.forEach((l, i) => after(() => l.classList.add("draw"), 120 + i * 22));
        g.nodeEls.forEach((n, i) => after(() => n.classList.replace("seed", "pop"), 220 + i * 26));
        after(() => mini.classList.add("float"), 220 + g.nodeEls.length * 26 + 600);
      }

      let cancelled = false;
      (async function boot() {
        const heatEls = buildHeat();
        const graph = buildGraph();
        const inner = root.querySelector(".home-inner");
        const blocks = [...inner.children];
        blocks.forEach((b) => b.classList.add("reveal"));
        const reveal = (el) => el && el.classList.add("in");
        const qa = (s) => root.querySelector(s);

        if (reduce) {
          blocks.forEach(reveal);
          root.querySelectorAll(".am-val,.stat-val").forEach((e) => countUp(e));
          revealHeat(heatEls); revealGraph(graph);
          root.querySelectorAll(".prog-fill").forEach((f) => f.classList.add("grow"));
          const agi = qa(".act-goal .ag-bar > i"); if (agi) agi.classList.add("grow");
          root.querySelectorAll(".h-card.ai").forEach((c) => prepAICard(c, "")());
          inner.classList.remove("booting");
          return;
        }

        const aiCards = [...root.querySelectorAll(".h-card.ai")];
        const aiDone = aiCards.map((c) => prepAICard(c, lang === "en" ? "Analyzing notes…" : "Анализирую заметки…"));

        await wait(120); if (cancelled) return;
        reveal(blocks[0]); await wait(110);
        reveal(blocks[1]); await wait(120);
        reveal(blocks[2]); await wait(130);
        reveal(blocks[3]); await wait(160); if (cancelled) return;

        reveal(blocks[4]); await wait(80); reveal(blocks[5]); await wait(120);
        after(() => aiDone[0] && aiDone[0](), 950);

        reveal(blocks[6]); await wait(80); reveal(blocks[7]);
        root.querySelectorAll(".act-metric .am-val").forEach((e, i) => after(() => countUp(e), 250 + i * 110));
        revealHeat(heatEls);
        after(() => { const n = root.querySelector("#streak-num"); if (n) countUp(n, 1100); }, 60);
        after(() => { const b = qa(".act-goal .ag-bar > i"); b && b.classList.add("grow"); }, 500);
        revealGraph(graph);
        await wait(260); if (cancelled) return;

        reveal(blocks[8]); await wait(80); reveal(blocks[9]);
        root.querySelectorAll(".prog-fill").forEach((f, i) => after(() => f.classList.add("grow"), 200 + i * 130));
        root.querySelectorAll(".stat-val").forEach((e, i) => after(() => countUp(e), 200 + i * 90));
        await wait(240); if (cancelled) return;

        reveal(blocks[10]); await wait(80); reveal(blocks[11]);
        after(() => aiDone[1] && aiDone[1](), 800);
        await wait(220); if (cancelled) return;

        reveal(blocks[12]); await wait(80); reveal(blocks[13]);
        after(() => aiDone[2] && aiDone[2](), 850);
        inner.classList.remove("booting");
      })();

      // ── click wiring ──
      const onClick = (e) => {
        if (e.target.closest(".continue")) return onOpenNote("rag-pipeline");
        if (e.target.closest(".home-search")) return onSearch();
        const qaBtn = e.target.closest(".qa");
        if (qaBtn) {
          const act = qaBtn.getAttribute("data-act");
          if (act === "graph") return onGraph();
          if (act === "reindex") return toast(lang === "en" ? "Re-indexing started…" : "Запущена переиндексация…");
          return onNewNote();
        }
        if (e.target.closest(".graph-card")) return onGraph();
        const row = e.target.closest("[data-note]");
        if (row) return onOpenNote(row.getAttribute("data-note"));
        if (e.target.closest(".oq")) return onSearch();
      };
      root.addEventListener("click", onClick);

      return () => { cancelled = true; timers.forEach(clearTimeout); root.removeEventListener("click", onClick); };
    }, [lang]);

    return React.createElement("main", { className: "home", ref: rootRef },
      React.createElement("div", { className: "home-inner booting", dangerouslySetInnerHTML: { __html: INNER } }));
  }
  window.Home = Home;
})();
