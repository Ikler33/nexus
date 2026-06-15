# Дизайн-fidelity аудит Nexus ↔ хэндофф «Hermes» — 2026-06-15

> Мультиагентная сверка (10 экранов, по агенту на экран; read-only). **Итог: приложение в сильном паритете с макетом и во многом ВПЕРЕДИ** — токен-слой идентичен (палитра/шрифты/радиусы/motion), расхождения структурные/композиционные. 55 гэпов: 37 не меняют дирекцию, 18 owner-gated.

> Источник анализа: workflow `design-fidelity-sweep` (wf_7556c7f4-d2a). Синтез-агент дал деградированный вывод — отчёт пересобран из per-screen структур вручную.


## Вердикты по экранам

- **app-shell** — minor-drift (9 гэпов)
- **sidebar** — close (2 гэпов)
- **editor** — minor-drift (8 гэпов)
- **ai-panel** — minor-drift (12 гэпов)
- **palette** — minor-drift (3 гэпов)
- **graph** — close (1 гэпов)
- **onboarding** — minor-drift (8 гэпов)
- **plugins** — notable-drift (7 гэпов)
- **conflict** — notable-drift (6 гэпов)
- **tweaks (Settings)** — close (4 гэпов)

## A. Autonomous-safe — доводки по существующему макету (чиним без апрува)

- **[med/MISSING-IN-APP] onboarding — Отсутствует опция «Создать новое хранилище» на Vault Step**
  - Макет содержит 3 опции выбора vault: 1) Открыть папку 2) Создать новое 3) Демо. Живой код реализует только 1 и условно 3 (только вне Tauri). Опция «Создать новое» отсутствует, хотя логика openVaultFlow() ее поддерживает (системный диалог позволяет создать папку).
  - живой: `Onboarding.tsx:198–224` · фикс: Добавить вторую кнопку-опцию для создания нового vault (иконка plus, текст как в i18n.onboarding.vault_new). Вызов тот же openVaultFlow() с флагом create=true.
- **[med/MISSING-IN-APP] onboarding — AI Step: отсутствует кнопка «Пока без AI» (skip AI)**
  - Макет имеет Ghost-кнопку 'Пока без AI' (ai_skip) между Back и Continue. Живой код не предусматривает skip — пользователь обязан либо вернуться (Back), либо продолжить с текущей конфигурацией. Логика skipping в макете: setAi(null) → setStep(3).
  - живой: `Onboarding.tsx:226–251 (actions: только Back + Primary)` · фикс: Добавить третью кнопку в .actions перед Primary с onClick={() => { setAi(null); setStep('index'); }} и text из i18n.onboarding.aiSkip.
- **[med/RECONCILE] ai-panel — Бейдж провайдера: cloud-вариант в макете, offline в приложении без cloud**
  - Макет предусматривает cloud-вариант провайдера (cloud fallback, срез 3 по плану). Приложение показывает только local (зелёная drive-иконка) и offline (warning wifi-off). Cloud-вариант зарезервирован (E9, комментарий в коде), но не реализован. Это ожидается.
  - живой: `/Users/artem/Documents/NEXUS-be/apps/desktop/src/components/chat/AiPanel.tsx:33-56` · фикс: Не требует фикса — cloud-поддержка планируется. Макет корректен. Приложение пока показывает только local/offline.
- **[med/RECONCILE] plugins — Consent persistence & display model differs**
  - Mockup shows modal consent-sheet (centered, full disclosure, Allow/Cancel buttons). Live shows inline 'Consent given · [Revoke]' text below permission chips after user grants once. Mockup is pre-install consent flow; live is persistent-grant-with-revoke-option flow. Different UX semantics and discoverability of revocation.
  - живой: `PluginsPanel.tsx lines 58–103, lines 188–199: localStorage-persisted consent map, inline display as 'Consent given' + revoke button` · фикс: Either (a) keep modal consent in mockup and add localStorage recall in live, or (b) update mockup to show inline 'Consent given' state with revoke link after first grant.
- **[med/MISSING-IN-APP] palette — Кнопка очистки поля (Clear button) отсутствует**
  - В макете: если q не пусто, вместо kbd(Esc) показывается button.tb-btn с иконкой ×. В приложении: всегда показывается только kbd(Esc), нет интерактивной кнопки очистки.
  - живой: `CommandPalette.tsx:276` · фикс: Добавить условный рендер кнопки: `q ? <button onClick={() => setQuery('')}><X size={14}/></button> : <kbd>Esc</kbd>`
- **[med/RECONCILE] conflict — Явный textarea для ручной правки результата вместо скрытого результата**
  - Макет ориентирован на выбор одной из трёх сторон (local/remote/both) и автоматическое слияние. Приложение добавило UX для ручного редактирования результата через .result textarea (с состоянием 'manual'). Это функция, которой в макете нет, но приложение правильно её реализовало. Однако она занимает место в каждом ханке и нарушает компактность макетного вида.
  - живой: `ConflictResolver.tsx:234–242 — <textarea value={...resolutionFor(f)} onChange={setManual}> в каждом .file; lado.module.css:167–183 .result с resize + focus-стили` · фикс: Либо добавить textarea в макет как опциональное дополнение для power-users, либо скрыть по умолчанию (toggle 'Редактировать' или прощёлкивание правой кнопкой). Сейчас это удобный фикс, но не совпадает с макетной UX.
- **[med/POLISH] app-shell — Разделитель в титлбаре и порядок кнопок**
  - В макете разделитель (tb-divider) только перед traffic-lights на Windows (line 66). В приложении разделитель есть между меню AI-инсайтов и режимом чтения (читаемость отделена иконкой-слэша). Порядок: макет → reading·graph·RU/EN·theme·AI; приложение → AI-меню▾·|·reading·RU/EN·theme·AI-panel. Визуально оправданно (меню отделено), но структурно отличается.
  - живой: `Titlebar.tsx line 137: divider между AI-меню и читаемостью` · фикс: Если AI-инсайты остаются в меню: оставить разделитель перед reading как в приложении (minor polish). Иначе переделать titлбар полностью (см. пункт 1).
- **[low/POLISH] onboarding — Vault Step: отсутствует visual feedback (checkmark) при выборе опции**
  - Макет показывает checkmark-иконку в выбранной опции (value === o.id ? Icon(check)). Живой код не рендерит checkmark — опция кликабельна, но нет визуального подтверждения выбора.
  - живой: `Onboarding.tsx:204–221` · фикс: В конце .opt рендер: {/* to add after optGo */} условное ChevronRight или Check иконку при условии того, что опция выбрана (добавить state-логику выбора).
- **[low/POLISH] onboarding — Index Step complete: отсутствует BrandMark на кнопке «Открыть Nexus»**
  - Макет: кнопка содержит иконку BrandMark (20px, радиус 6) слева от текста 'Открыть Nexus'. Живой код: только текст без иконки.
  - живой: `Onboarding.tsx:266–274 (button without BrandMark, only ChevronRight implicit)` · фикс: Добавить <BrandMark size={20} /> перед текстом в кнопку. CSS уже имеет gap и flex для inline-иконок.
- **[low/RECONCILE] editor — Mode-float позиция Y: top:10px vs top:50px**
  - Макет: mode-float пилюля top:50px (середина экрана +-). Живой код: top:10px (у верхнего края контейнера scroll). Позиция Y отличается — в живом коде ближе к верхнему краю.
  - живой: `/Users/artem/Documents/NEXUS-be/apps/desktop/src/components/workspace/GroupPane.module.css:220–221` · фикс: Изменить top: 10px на top: 50px в GroupPane.module.css:220 если нужно совпадение с макетом, или оставить 10px (более практично для большого скролла).
- **[low/POLISH] ai-panel — Stop-кнопка: цвет и стиль расходятся**
  - Макет: stop-кнопка — квадратная 32×32 иконочная с danger-фоном и белым цветом. Приложение: кнопка-пилюля с text-лейблом 'Стоп', danger-soft фоном и danger-цветным текстом (color-mix). Визуально приложение мягче (soft-фон), макет жёстче (solid danger). Также приложение показывает текст, макет — только иконку.
  - живой: `/Users/artem/Documents/NEXUS-be/apps/desktop/src/components/chat/ChatPanel.module.css:408-410` · фикс: Либо синхронизировать иконку в приложении (убрать текст), либо обновить макет под реальный text-вариант. Цвет опционально привести в соответствие (danger-soft vs solid).
- **[low/RECONCILE] ai-panel — AI-ответ: размер шрифта разный (text-sm vs text-md)**
  - Макет задаёт text-md (больший размер) для пузырей сообщений. Приложение использует text-sm (line 211), что меньше. Это воспринимается как более плотная и менее воздушная типография ответов ИИ.
  - живой: `/Users/artem/Documents/NEXUS-be/apps/desktop/src/components/chat/ChatPanel.module.css:211` · фикс: Обновить ChatPanel.module.css line 211: измените font-size на text-md или приведите макет к text-sm. Рекомендуется text-md для читаемости длинных ответов.
- **[low/POLISH] ai-panel — Композер: rows-атрибут совпадает, но padding/alignment может отличаться**
  - Макет: rows=1 (однострочный композер). Приложение: rows=2 (двухстрочный для удобства). Макет заказывает более компактный вариант, приложение расширило для UX (фидбэк 11.06).
  - живой: `/Users/artem/Documents/NEXUS-be/apps/desktop/src/components/chat/ChatPanel.tsx:333` · фикс: Этот выбор обоснован (rows=2 лучше для многострочного ввода). Обновить макет на rows=2 или оставить как есть, если требуется компактность.
- **[low/POLISH] ai-panel — Источники: макет показывает второй сниппет-строку в cards, приложение — только название + контекст**
  - Макет: в cards-варианте источников показывает номер + заголовок + контекст (snippet второй строкой). Приложение: Sources функция (ChatView:373-425) корректно рендерит оба (srcCardTitle + srcCardCtx). Совпадает. Но гибридный вариант (chips + cards) может отличаться от чистого макета.
  - живой: `/Users/artem/Documents/NEXUS-be/apps/desktop/src/components/chat/ChatPanel.module.css:412-425` · фикс: Стили совпадают. Возможная несостыковка в логике отображения — проверить, показываются ли контекст-сниппеты в реальном приложении при трёх вариантах отображения (chips/footnotes/cards).
- **[low/POLISH] plugins — Permission chips missing icons**
  - Mockup's permission chips include an icon (file-text, globe, terminal, etc.) + label. Live chips are text-only (permission label only, no icon). Visual polish difference: icons make chip semantics clearer at a glance.
  - живой: `PluginsPanel.module.css lines 255–276, .permChip: text-only, no icon child element; PluginsPanel.tsx lines 115–123` · фикс: Add icon import + render `<Icon name={PERM_ICON[c.kind]} size={11} />` before the permission label in the chip.
- **[low/POLISH] plugins — Consent modal styling: size and padding**
  - Mockup doesn't specify exact consent-sheet dimensions (CSS is embedded in JSX). Live uses 460px max-width with responsive scaling. Both use centered modal + scrim overlay, so semantically aligned, but live's explicit sizing differs from mockup implicit sizing.
  - живой: `PluginsPanel.module.css lines 319–347: .consent width min(460px, 92%), padding 20px` · фикс: Verify mockup consent-sheet is visually similar width (should be ~460–500px for two-column layout); if different, align sizes. Low priority—both are visually comparable.
- **[low/POLISH] sidebar — Звёзды в дереве файлов: видимость на hover вместо всегда**
  - В макете звезда рендерится условно (если заметка отмечена); в приложении звезда всегда в DOM, но скрыта (opacity: 0) до hover или включения. Это влияет на видимость в спокойном состоянии — в макете пусто, в приложении пусто но потом видна на hover.
  - живой: `FileTree.tsx строки 231–245 — button.star с opacity: 0 до hover/data-on` · фикс: Либо всегда показывать звёзды (удалить opacity: 0 из baseline), либо совпадать с макетом (не рендерить звезду если не включена).
- **[low/POLISH] sidebar — StarredPanel: отсутствует twist-спейсер перед иконкой**
  - Макет использует класс tree-row и включает пустой twist-элемент для выравнивания (как в FileTree с иконкой раскрывателя). Приложение использует tagRow (как в TagsPanel) и не включает спейсер. Визуально не критично (это разные панели, переключаемые по tab), но нарушает паттерн выравнивания если их рядом.
  - живой: `Sidebar.tsx строки 333–353 — button.tagRow без спейсера перед FileText` · фикс: Добавить пустой спейсер (span aria-hidden) перед иконкой в StarredPanel для выравнивания как в дереве.
- **[low/RECONCILE] palette — Динамическая иконка inputRow (command в spotlight, search в других) не реализована**
  - Макет: иконка в .cmd-input-row меняется: command (20px) в стиле spotlight, search (17px) в остальных. Приложение: всегда Search (16px), независимо от paletteStyle.
  - живой: `CommandPalette.tsx:260` · фикс: Подставить динамическую иконку: `{paletteStyle === 'spotlight' ? <CommandIcon size={20}/> : <Search size={16}/>}`
- **[low/MISSING-IN-APP] palette — Третья hint-группа в футере (бренд Nexus) отсутствует**
  - Макет: footer содержит 3 span.kb-hint: (↑↓, ↵) слева, затем (с marginLeft auto) command-иконка + 'Nexus' справа. Приложение: только 2 hint-группы без третьей.
  - живой: `CommandPalette.tsx:312-319 (весь .foot)` · фикс: Добавить span с marginLeft: auto и CommandIcon (size 11) + 'Nexus' текстом в конец .foot.
- **[low/RECONCILE] tweaks (Settings) — Density selector has 3 options instead of 2**
  - Spec defines density as binary (comfortable/compact), but live implementation offers 3 options: comfortable, compact, and auto. The 'auto' mode is an enhancement that was added during development but not reflected in the original design spec.
  - живой: `/Users/artem/Documents/NEXUS-be/apps/desktop/src/components/settings/SettingsView.tsx:282-306` · фикс: Either update the design spec to document the 3-option density selector, or confirm that 'auto' mode is intentional and should remain (recommended: keep, it's a useful feature). No code change needed.
- **[low/RECONCILE] tweaks (Settings) — AI section has 3 endpoints instead of 2**
  - Spec shows Chat and Embedding endpoints; live adds a third 'Fast' endpoint (for fast inference). This is a feature expansion (per note that 'приложение ушло вперёд макета'), but the design spec should be updated to reflect reality.
  - живой: `/Users/artem/Documents/NEXUS-be/apps/desktop/src/components/settings/SettingsView.tsx:457-486` · фикс: Update design spec to show 3 endpoints: Chat, Embedding, Fast. Code is correct — no changes needed.
- **[low/POLISH] conflict — Прогресс отображается в шапке вместо футера**
  - Макет располагает прогресс-счётчик в футере, под прогресс-баром. Приложение показывает его в шапке в виде «Разрешено N из M» (мотнозначный текст, фаза мuted). Разница небольшая, но нарушает баланс макета (шапка перегружена).
  - живой: `ConflictResolver.tsx:125–128 — .progressLabel в .header (справа, после .title)` · фикс: Переместить .progressLabel в футер (вместе с .bar и кнопками).
- **[low/POLISH] conflict — Отсутствует checkmark-иконка при выборе стороны в визуально очевидном месте**
  - Макет показывает checkmark внутри кликабельной stacked-стороны (`.cfl-side`) как подтверждение выбора. Приложение рендерит её в `.sideLabel` (абсолютная позиция вверху), что менее очевидно из-за компактности. Плюс макетный checkmark на белом фоне с акцентным цветом более заметен.
  - живой: `ConflictResolver.tsx:193, 205 — <Check size={12}/> рендерится внутри .sideLabel (в абсолютной позиции сверху), но в CSS .sideLabel это минимальный хедер текста, не очевидный контейнер для иконки` · фикс: Добавить .checkIco в CSS с лучшей видимостью, или перенести иконку в конец .sideText вместо абсолютной позиции.
- **[low/POLISH] app-shell — Язык-кнопка: разделитель RU·EN vs RU/EN**
  - Макет: RU / EN (слэш как разделитель). Приложение: RU · EN (центральная точка). Это мелкий тикет, логика одна, но визуальное оформление другое. Токены совпадают (цвет, размер).
  - живой: `Titlebar.tsx line 157: className 'sep' → '·'` · фикс: Выбрать унифицированный разделитель (· или /) и применить везде. Сейчас в приложении ·, это нейтрально, но макет говорит /.
- **[low/POLISH] app-shell — BrandMark size: 26px (макет) vs 24px (приложение)**
  - Размер иконки бренда немного отличается. Макет: 26px, приложение: 24px. Мелкая польша, визуально почти неотличима.
  - живой: `Titlebar.tsx line 92: BrandMark size 24` · фикс: Согласовать размер с приложением (24px) или обновить CSS в приложении на 26px из макета. Токены не определены, это инлайн-значение.
- **[low/MISSING-IN-APP] app-shell — Status bar: AI-инсайты отсутствуют**
  - Статусбар в приложении не содержит быстрых ссылок на Дайджест/Цели/Противоречия. Те вызываются из меню sparkles▾ в титлбаре или кнопок ActivityBar. Макет не специфицирует, будут ли они в статусбаре, но логично добавить их туда для быстрого доступа (как в приложении для sparkles-меню). Это не критично, т.к. входы есть (в меню), но статусбар потенциально мог бы показать, есть ли новые инсайты (бейджи на кнопках).
  - живой: `StatusBar.tsx line 23–198: только синхро/индексация/конфликт, нет прямого входа к Дайджесту/Целям` · фикс: Опционально: добавить бейджи/индикаторы на кнопках sparkles▾ в титлбаре, если есть новые Дайджесты/Цели/Противоречия. Сейчас это работает (панели открываются), но нет сигнала о новых данных.
- **[low/POLISH] app-shell — Порядок кнопок в группе (theme → язык vs язык → theme)**
  - Макет: читаемость → граф → RU/EN → тема → AI. Приложение: читаемость → RU/EN → тема → AI-панель. Порядок немного отличается (граф переехал в ActivityBar), но это следствие архитектурного решения, а не ошибка вёрстки.
  - живой: `Titlebar.tsx line 139–180: reading·lang·theme·AI-panel` · фикс: Порядок органичен, не требует исправления. Это следствие перестройки хрома (см. пункт 1).

## B. Owner-gated — решения о ВИЗУАЛЬНОЙ ДИРЕКЦИИ (нужен апрув ПЕРЕД постройкой)

- **[high/RECONCILE] onboarding — AI Step: полностью переработана логика — вместо выбора (local/cloud) только отображение конфига**
  - Макет предусматривает выбор между локальной моделью (llama.cpp, иконка drive) и облачным провайдером (иконка cloud) как две отдельные clickable опции-карточки (.onb-opt) с checkmark при выборе. Живой код: 1) читает .nexus/local.json 2) показывает одну строку (aiRow) с иконкой Cpu и статусом конфига 3) нет UI выбора между local/cloud 4) нет checkmark. Структурно это разные подходы: макет — wizard-выбор, живой — конфиг-ревью.
  - живой: `Onboarding.tsx:226–251` · фикс: Owner-decision: либо вернуть к макетному выбору (2 опции, toggleable), либо обновить макет под реальность (одна инфо-строка с health-статусом). Рекомендация: оставить живой UX (проще, честнее), макет обновить.
- **[high/MISSING-IN-APP] plugins — Missing Marketplace browse tab**
  - Mockup shows two tabs: 'Установленные' (installed plugins with manage UI) and 'Маркетплейс' (browsable catalog of !installed plugins with install button). Live implementation has only 'Installed' (with launch button) + 'Sandbox' (demo iframe + audit log). The marketplace browsing feature is completely absent. This is a STRUCTURAL difference: users cannot discover/install new plugins from the UI.
  - живой: `PluginsPanel.tsx lines 56, 168–212: only 'installed' tab is present; 'sandbox' tab is new` · фикс: Either (a) add a 'Marketplace' tab that renders uninstalled plugins with 'Install' button, or (b) accept that mockup marketplace was aspirational and update mockup to match live runtime model (launch→sandbox+audit only).
- **[high/RECONCILE] plugins — Card action button pattern mismatch**
  - Mockup shows toggle switch + remove button for installed plugins, OR install button for marketplace plugins. Live shows single 'Launch' button (disabled if incompatible) with no toggle/remove visible. Semantically: mockup enables/disables via toggle; live 'launches' (navigates to sandbox). This reflects different UX models: mockup = persistent enable/disable toggle; live = one-shot launch to demo.
  - живой: `PluginsPanel.tsx lines 172–210: all cards show 'Launch' button; 'Installed' tab only (no uninstalled cards); disabled if !compatible` · фикс: Clarify product intent: (1) if mockup model desired, add toggle+remove buttons back, (2) if live model is intentional, update mockup cards to show 'Launch' button instead of toggle.
- **[high/MISSING-IN-APP] conflict — Отсутствует правый рельс с навигацией конфликтов и статистикой**
  - Макет предусматривает правый рельс (вертикальная полоса ~120–140px) с (а) статистикой правок (2 бокса), (б) списком конфликтов с кликабельной навигацией и прыжком+flash-анимацией, (в) bulk-кнопками. В приложении все элементы распределены по телу (bulk в начале, прогресс в шапке, нет навигатора). Это кардинальное переоформление диалога: вместо 3-колоночной сетки (head | doc | rail / foot) приложение использует одну вертикальную ленту (head | body | apply-кнопка).
  - живой: `ConflictResolver.tsx — нет элемента, соответствующего .cfl-rail` · фикс: Либо добавить правый рельс (CSS grid в .dialog: grid-template-columns 1fr minmax(140px,auto), перенести .bulk/.progressLabel в .cfl-rail), либо явно решить, что приложение отходит от макета в пользу компактной мобильной версии (тогда обновить макет).
- **[high/MISSING-IN-APP] conflict — Отсутствует футер с прогресс-баром и глобальными кнопками действия**
  - Макет имеет чёткий футер с прогресс-баром и 2 кнопками. В приложении: (а) прогресс показан в .progressLabel в шапке (только текст), (б) Apply-кнопка находится в конце тела (после всех файлов), (в) кнопка Отмена отсутствует (диалог закрывается по клику на backdrop). Нарушает макетное разделение зон.
  - живой: `ConflictResolver.tsx:246–254 — .applyBtn плавает в конце .body вместо футера; нет прогресс-бара; нет кнопки Отмена` · фикс: Добавить .footer с прогресс-баром + кнопкой Close (вместо Close в .header) и Apply (зачем дублировать в теле).
- **[high/RECONCILE] app-shell — Структура хрома переходит на ActivityBar**
  - Макет концентрирует ВСЕ управляющие кнопки в горизонтальном титлбаре (panel-left / читаемость / граф / RU/EN / тема / AI). Приложение переместило большинство во вертикальный ActivityBar слева (Home / News / Файлы / Граф / Задачи / Inbox + внизу Синхро / Настройки), оставив в титлбаре только поиск + AI-меню▾ (раскрывающееся) + читаемость + RU/EN + тема + AI-панель. Это значит: (a) кнопка panel-left НЕ в титлбаре, а в ActivityBar; (b) граф — в ActivityBar, не в титлбаре; (c) настройки/синхро — в ActivityBar, не в титлбаре; (d) AI-функции (Дайджест/Цели/Противоречия) за раскрывающимся меню sparkles▾, а не отдельные кнопки.
  - живой: `App.tsx line 223–224, Titlebar.tsx line 42–181, ActivityBar.tsx line 21–97` · фикс: Обновить макет app.jsx: (1) убрать из Titlebar кнопки panel-left, граф, синхро, настройки, дайджест, цели, противоречия; (2) добавить вертикальный компонент ActivityBar (как на макете); (3) переместить граф/задачи/inbox в ActivityBar; (4) AI-меню (sparkles▾) с выпадающей меню дайджест/цели/противоречия; (5) оставить в титлбаре поиск/читаемость/RU/EN/тема/AI-панель как сейчас в реальности.
- **[med/RECONCILE] graph — Лейблы узлов видны условно вместо всегда (zoom-dependent + active/hover)**
  - Макет показывает текстовый лейбл под каждым узлом всегда. Приложение отображает лейблы только в трёх случаях: (1) активный узел, (2) hover, (3) масштаб 1.25–3.2. На уровне макета (900px) при загрузке лейблы не видны, что снижает разборчивость графа.
  - живой: `GraphView.tsx:714-739 (labelOn = pin || labelsByZoom; {labelOn && <text>...})` · фикс: Вернуть labelOn=true для всех узлов (удалить zoom-условие), или явно задокументировать, что это задизайнено для экономии пикселей при zoom-out.
- **[med/MISSING-IN-APP] ai-panel — Вкладка 'Похожие' отсутствует в макете, есть в приложении**
  - Макет предусматривает 3 вкладки: Chat, Suggestions, Summary. Приложение: Chat, Suggest, Related (вместо Summary). Вкладка Related (#35 в дизайне) добавлена в приложение, но Summary удалена (совпадает с предыдущим аудитом — суммаризация в inline-LLM редактора). Макет устарел.
  - живой: `/Users/artem/Documents/NEXUS-be/apps/desktop/src/components/chat/AiPanel.tsx:310-317` · фикс: Обновить макет: заменить Summary на Related; дизайн Related уже есть в коде (RelatedView.tsx). Это решение владельца, макет надо привести в соответствие.
- **[med/MISSING-IN-APP] ai-panel — Web-флаг и переключатель режима (Vault/General) отсутствуют в макете**
  - Макет показывает чат без видимого переключателя режима. Приложение добавило (фидбэк владельца 11.06): сегмент radio-group "По заметкам | Общий" + кнопка globe "Web". Это был осознанный апдейт, не ошибка — управляет RAG-контекстом и web-поиском.
  - живой: `/Users/artem/Documents/NEXUS-be/apps/desktop/src/components/chat/ChatPanel.module.css:310-346` · фикс: Обновить макет: добавить над композером сегмент с режимами (vault/general) и кнопку Web (глобус). Уже реализовано в коде.
- **[med/RECONCILE] ai-panel — Thinking-фаза: стриминг reasoningSummary не отражён в макете**
  - Макет показывает фазу мышления как BrandThinking-иконку + статичный label 'Думаю'. Приложение: BrandThinking + переливающийся label, в который СТРИМЯТСЯ сводки CoT (reasoningSummary от R1 модели). Это честная фича приложения, макет упрощённый.
  - живой: `/Users/artem/Documents/NEXUS-be/apps/desktop/src/components/chat/ChatPanel.module.css:156-184` · фикс: Обновить макет: добавить в label-анимацию streaming (shimmer/gradient) или указать текст сводки CoT. Стриминг уже реализован в коде.
- **[med/RECONCILE] app-shell — Иконка AI в титлбаре: sparkles vs меню▾**
  - Макет: простая кнопка sparkles (toggle AI-панель). Приложение: sparkles▾ (выпадающее меню для Дайджест/Цели/Противоречия) + отдельная кнопка PanelRight для включения/выключения панели. Это связано с переводом AI-функций в ActivityBar и группированием в меню (как в DP-4 ревью). Структурно правильно (кнопок много, меню органично), но макет не соответствует.
  - живой: `Titlebar.tsx line 106–135: aiWrap, sparkles + ChevronDown, раскрывающееся меню` · фикс: Обновить макет: sparkles▾ с раскрывающимся меню (Дайджест / Цели / Противоречия), плюс отдельная кнопка panel-right для AI-панели (как в приложении сейчас).
- **[med/RECONCILE] app-shell — Кнопка сворачивания сайдбара (panel-left) в ActivityBar, не в титлбаре**
  - Макет помещает panel-left в титлбар слева (первая кнопка, toggle sidebar). Приложение: кнопка FileText в ActivityBar (вторая в верхней группе после Home/News). Это архитектурное переустройство — сайдбар-тоггл логически принадлежит ActivityBar, а не титлбару (как в VS Code / Obsidian).
  - живой: `ActivityBar.tsx line 61–65: FileText toggle sidebarOpen` · фикс: Обновить макет: убрать panel-left из Titlebar, добавить в ActivityBar как FileText (тогда переименовать в панели-макета из panel-left на file-text для соответствия иконографии lucide).
- **[low/RECONCILE] onboarding — Welcome → Vault: опция «Demo vault» условная (только вне Tauri)**
  - Макет показывает опцию Demo всегда. Живой код: она видна только в браузере (!isTauri()). Это может быть feature, но в макете нет условности — либо макет неполон, либо это намеренное различие (в desktop-приложении demo не нужен, т.к. есть реальный vault).
  - живой: `Onboarding.tsx:212–221 ({!isTauri() && <button... demo})>` · фикс: Если demo нужен всегда: убрать !isTauri() guard. Если это правильное поведение: обновить макет с условностью либо добавить примечание, что demo только в web.
- **[low/MISSING-IN-APP] ai-panel — История сессий (SessionHistory) в приложении, в макете нет**
  - Приложение добавило кнопку-часы (History) в шапку панели, которая открывает glass-дропдаун с группировкой сессий по датам (Claude/ChatGPT-style). Макет этого не предусматривает. Это фича, не расхождение — приложение впереди.
  - живой: `/Users/artem/Documents/NEXUS-be/apps/desktop/src/components/chat/AiPanel.tsx:74-169` · фикс: Добавить в макет шапки панели кнопку-часы справа от sparkles-иконки заголовка (перед действиями New Session / Refresh / Close). Дизайн glass-дропдауна уже реализован в приложении.
- **[low/MISSING-IN-APP] ai-panel — Pin-чипы для закреплённых заметок отсутствуют в макете**
  - Приложение добавило ряд pin-чипов (P6-PIN) между режимом и композером — показывают закреплённые в контексте заметки. Макет этого не отражает. Фича, приложение впереди.
  - живой: `/Users/artem/Documents/NEXUS-be/apps/desktop/src/components/chat/ChatPanel.module.css:266-291` · фикс: Добавить в макет между modeRow и composer строку с pin-чипами (если заметки закреплены). Дизайн реализован в приложении.
- **[low/MISSING-IN-APP] ai-panel — Suggest-чипы для быстрого добавления в контекст отсутствуют в макете**
  - Приложение добавило ряд suggest-чипов (AIP-11) — кандидаты в контекст на основе семантической близости к открытой заметке. Макет не предусматривает. Фича, приложение впереди.
  - живой: `/Users/artem/Documents/NEXUS-be/apps/desktop/src/components/chat/ChatPanel.module.css:293-311` · фикс: Добавить в макет строку со средними pill-чипами (+ иконка + имя заметки) между pin-row и composer. Дизайн есть в коде.
- **[low/RECONCILE] ai-panel — Фон панели: макет указывает --color-chrome, приложение использует --color-bg-elevated**
  - Макет использует --color-chrome (более контрастный, принадлежит шапке и хрому). Приложение — --color-bg-elevated (приподнятый фон, более нейтральный). Визуально панель в приложении кажется светлее/теплее. Возможно, намеренное отклонение от макета.
  - живой: `/Users/artem/Documents/NEXUS-be/apps/desktop/src/components/chat/ChatPanel.module.css:7` · фикс: Проверить с владельцем: намеренное ли использование --color-bg-elevated? Если макет источник истины — вернуть --color-chrome. Если приложение — обновить макет.
- **[low/RECONCILE] conflict — Bulk-кнопки расположены в начале тела вместо правого рельса**
  - Макет размещает bulk-кнопки в правом рельсе вместе со статистикой и навигатором. Приложение разместило их в начале основного тела (перед списком файлов). Это изменение логики: в макете они «на краю», в приложении — «на переднем плане».
  - живой: `ConflictResolver.tsx:159–166 — <div className={styles.bulk}> с двумя <button> в начале .body, перед файлами` · фикс: Если добавить рельс (см. гэп 1), перенести .bulk туда. Если приложение намеренно выводит их на перед, нужен апрув.

## C. «Приложение впереди» — обновить МАКЕТ, не приложение

- **onboarding — Index Step: отсутствует счётчик прогресса (done / TOTAL chunks)** · Добавить div с текстом `${done} / ${TOTAL} ${t('onboarding.chunks')}` перед .progress. Макет: `done` и `TOTAL` считаются в компоненте IndexStep, живой код их уже имеет (done = counter при индексации).
- **onboarding — Index Step: отсутствует список обрабатываемых файлов** · Либо: 1) Добавить div.onb-files под прогресс-баром с map over `files` (которые уже в state); 2) либо обновить макет, убрав этот элемент (редкий паттерн для реального приложения). Рекомендация: добавить, если файлы действительно стримят с бэка.
- **editor — AppendLine отсутствует в живом коде** · Нужно добавить компонент AppendLine в GroupPane после MarkdownPreview (аналог макета lines 160 в preview-режиме). Создать компонент если не существует или подключить из макета.
- **editor — OutlineBar и MentionsBar добавлены поверх макета** · Это не гэп — это приложение впереди макета. Макет можно обновить, чтобы включить эти панели, или оставить как расширение приложения. Они не нарушают макет.
- **plugins — Incompatibility badge is new (not in mockup)** · Add optional incompatibility indicator to mockup for realism, or document that live compatibility-check is orthogonal to mockup.
- **plugins — Card error line is new (not in mockup)** · Update mockup to show optional error state in card (red danger text, e.g. 'Manifest parsing failed'), or accept as live-only robustness feature.
- **tweaks (Settings) — Web Search section not in original spec (added as enhancement)** · Add Web Search configuration section to design spec (or acknowledge it as a post-spec enhancement). Code is correctly implemented per egress policy.
- **tweaks (Settings) — Egress/Network policy section not in original spec** · Either update design spec to include egress section, or document as intentional post-spec addition. Code is well-implemented with proper i18n and toast feedback.
- **app-shell — Отсутствие traffic-lights на Mac в макете vs presence в приложении** · Это не гэп — Tauri автоматически обеспечивает traffic-lights. Макет показывает их для полноты проектирования, но в реальной Tauri-сборке они управляются операционной системой.
