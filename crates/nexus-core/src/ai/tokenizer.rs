//! Реальный BPE-токенайзер для оценки бюджета контекста LLM (P0-c).
//!
//! Раньше токены считал [`crate::chunker::WordTokenizer`] — `split_whitespace().count()`. На
//! кириллице это врёт в 1.5–2× (RU-строка из 14 слов → 24 BPE-токена на реальной модели), а
//! владелец пишет по-русски: эвристика по словам недооценивала бюджет → переполнение окна модели.
//!
//! [`QwenTokenizer`] загружает `tokenizer.json` РЕАЛЬНОЙ задеплоенной модели и считает токены ровно
//! как сервер (`tk.encode(text, /*add_special_tokens=*/false)` — без BOS, как `llama.cpp /tokenize`).
//!
//! # Какую модель таргетит встроенный токенайзер
//! Встроенный ассет `assets/qwen3_tokenizer.json.gz` — это `tokenizer.json` модели
//! **`Qwen/Qwen3.6-27B`** (база `unsloth/Qwen3.6-27B-MTP-GGUF`, развёрнута на `192.168.0.31:8080`).
//! Vocab = 248 044 (совпадает с живой моделью; старый Qwen3-8B-файл с ~150k врал на ~250k-модели).
//! Офлайн-гейт [`tokenizer::tests`] сверяет `count()` встроенного токенайзера с числами `/tokenize`
//! живой модели — при расхождении CI красный (значит ассет ≠ задеплоенная модель).
//!
//! # Как сменить модель в будущем (owner просил: смена должна быть очевидной)
//! Токенайзер выбирается так: `ai.tokenizer_path` в `.nexus/local.json` → если задан, грузим файл с
//! диска; иначе — встроенный ассет. То есть смена модели — это **файл + конфиг**, без правки кода:
//!   1. (рекомендуется) положить новый `tokenizer.json` рядом и прописать `ai.tokenizer_path` —
//!      мгновенно, без пересборки;
//!   2. (для нового встроенного дефолта) `gzip -9 tokenizer.json > assets/qwen3_tokenizer.json.gz`,
//!      обновить vocab-число в этом док-комментарии и golden-числа в [`tests`] под новую модель.
//!
//! # Fail-closed
//! Если ассет/файл не парсится — `warn!` + переход на консервативную эвристику по СКРИПТАМ
//! ([`HeuristicTokenizer`]): латиница ~4 симв/токен, кириллица ~3, CJK ~1.3. Оценка осознанно
//! ЗАВЫШАЕТ (лучше не влезть с запасом, чем переполнить окно). Без паники.

use std::sync::OnceLock;

use crate::ai::ChatMessage;
use crate::chunker::Tokenizer;

/// gz-сжатый `tokenizer.json` задеплоенной модели (Qwen3.6-27B), встроен в бинарь (~3.3 МБ gz).
/// Распаковывается один раз при первом обращении (см. [`embedded_json`]).
const EMBEDDED_TOKENIZER_GZ: &[u8] = include_bytes!("assets/qwen3_tokenizer.json.gz");

/// Распакованный JSON встроенного токенайзера — декомпрессия gz ровно один раз на процесс.
fn embedded_json() -> Option<&'static [u8]> {
    static CACHE: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            use std::io::Read;
            let mut gz = flate2::read::GzDecoder::new(EMBEDDED_TOKENIZER_GZ);
            let mut out = Vec::new();
            match gz.read_to_end(&mut out) {
                Ok(_) => Some(out),
                Err(e) => {
                    tracing::warn!(error = %e, "не удалось распаковать встроенный токенайзер — fallback на эвристику");
                    None
                }
            }
        })
        .as_deref()
}

/// Готовый встроенный [`tokenizers::Tokenizer`] — парсинг JSON ровно один раз на процесс.
fn embedded_tokenizer() -> Option<&'static tokenizers::Tokenizer> {
    static CACHE: OnceLock<Option<tokenizers::Tokenizer>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let json = embedded_json()?;
            match tokenizers::Tokenizer::from_bytes(json) {
                Ok(tk) => Some(tk),
                Err(e) => {
                    tracing::warn!(error = %e, "не удалось разобрать встроенный токенайзер — fallback на эвристику");
                    None
                }
            }
        })
        .as_ref()
}

/// Реальный BPE-токенайзер задеплоенной модели. Держит либо загруженный с диска токенайзер
/// (`ai.tokenizer_path`), либо ссылается на встроенный. При неудаче загрузки — fail-closed на
/// эвристику (поле `tk` = `None`), считаем по [`HeuristicTokenizer`].
pub struct QwenTokenizer {
    /// `Some` — токенайзер из файла (`ai.tokenizer_path`); `None` — используем встроенный
    /// (`embedded_tokenizer()`), а если и тот не загрузился — эвристику.
    tk: Option<tokenizers::Tokenizer>,
    /// Консервативный fallback (всегда готов, без аллокаций).
    heuristic: HeuristicTokenizer,
}

impl QwenTokenizer {
    /// Токенайзер на ВСТРОЕННОМ ассете задеплоенной модели (дефолт). Декомпрессия+парсинг
    /// мемоизированы — конструктор дёшев, можно звать на каждый чанкинг.
    pub fn embedded() -> Self {
        // Прогреваем кэш заранее (warn! при сбое логируется один раз), но саму ссылку не храним:
        // `count()` берёт `embedded_tokenizer()` (тот же кэш). `tk=None` → встроенный/эвристика.
        let _ = embedded_tokenizer();
        Self {
            tk: None,
            heuristic: HeuristicTokenizer,
        }
    }

    /// Токенайзер из файла на диске (`ai.tokenizer_path`) — для смены модели без пересборки.
    /// При сбое чтения/парсинга — `warn!` + fail-closed на встроенный/эвристику.
    pub fn from_file(path: &std::path::Path) -> Self {
        match tokenizers::Tokenizer::from_file(path) {
            Ok(tk) => Self {
                tk: Some(tk),
                heuristic: HeuristicTokenizer,
            },
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "не удалось загрузить ai.tokenizer_path — fallback на встроенный токенайзер"
                );
                Self::embedded()
            }
        }
    }

    /// Выбор токенайзера: явный конфиг (`tokenizer_path`) → файл; иначе встроенный.
    pub fn from_config(tokenizer_path: Option<&std::path::Path>) -> Self {
        match tokenizer_path {
            Some(p) => Self::from_file(p),
            None => Self::embedded(),
        }
    }

    /// Активный токенайзер: загруженный из файла ИЛИ встроенный. `None` — оба недоступны (эвристика).
    /// Явный `match` (не `or_else`): встроенный — `&'static`, файловый — заём `&self`; объединяем в
    /// лайфтайм `&self`, что для `'static` всегда валидно.
    fn active(&self) -> Option<&tokenizers::Tokenizer> {
        match self.tk.as_ref() {
            Some(tk) => Some(tk),
            None => embedded_tokenizer(),
        }
    }
}

impl Tokenizer for QwenTokenizer {
    fn count(&self, text: &str) -> usize {
        match self.active() {
            // add_special_tokens=false → без BOS/EOS, ровно как `llama.cpp /tokenize`.
            Some(tk) => match tk.encode(text, false) {
                Ok(enc) => enc.len(),
                Err(e) => {
                    tracing::warn!(error = %e, "encode() упал — fallback на эвристику для этого текста");
                    self.heuristic.count(text)
                }
            },
            None => self.heuristic.count(text),
        }
    }
}

/// Консервативный fallback-токенайзер (fail-closed): оценивает число токенов по СКРИПТУ символов,
/// осознанно ЗАВЫШАЯ (чтобы не переполнить окно модели). Используется, если реальный токенайзер не
/// загрузился. Детерминирован, без зависимости от модели и без паники.
pub struct HeuristicTokenizer;

impl Tokenizer for HeuristicTokenizer {
    fn count(&self, text: &str) -> usize {
        // Скрипт-aware: разные письменности дают разное число символов на BPE-токен. Берём
        // КОНСЕРВАТИВНЫЕ (низкие) делители → оценка токенов скорее завышена, чем занижена.
        let mut latin = 0usize; // ASCII/латиница/цифры/пунктуация — ~4 симв/токен
        let mut cyr = 0usize; // кириллица — ~3 симв/токен (плотнее режется)
        let mut cjk = 0usize; // CJK — ~1.3 симв/токен (часто 1 символ ≈ 1 токен)
        for ch in text.chars() {
            if ch.is_whitespace() {
                continue;
            }
            match ch as u32 {
                0x0400..=0x052F => cyr += 1, // кириллица (основная + дополнение)
                0x4E00..=0x9FFF | 0x3040..=0x30FF | 0xAC00..=0xD7AF => cjk += 1, // CJK/кана/хангыль
                _ => latin += 1,
            }
        }
        // div_ceil — округление вверх (консервативно). +1 за каждую группу с символами уже учтён ceil.
        let est = latin.div_ceil(4) + cyr.div_ceil(3) + (cjk * 10).div_ceil(13);
        // Непустой текст → минимум 1 токен (избегаем 0 на строке из одной буквы).
        if est == 0 && text.chars().any(|c| !c.is_whitespace()) {
            1
        } else {
            est
        }
    }
}

/// Бюджет контекстного окна модели для сборки сообщений чата (P0-c).
///
/// Окно берётся из `ChatConfig.context_window` (`.nexus/local.json`) — НЕ хардкод: 32k сейчас,
/// 256k позже = только конфиг. Резерв `reserve_output` оставляет место под ОТВЕТ модели (генерация
/// тоже ест окно). [`fit`] укладывает сообщения в бюджет, сохраняя system-промпт и самые свежие
/// реплики, отбрасывая САМЫЕ СТАРЫЕ не-system (типичная стратегия скользящего окна диалога).
#[derive(Debug, Clone, Copy)]
pub struct ContextBudget {
    /// Полное контекстное окно модели в токенах (из `ChatConfig.context_window`).
    pub context_window: usize,
    /// Сколько токенов зарезервировать под ответ модели (вычитается из окна).
    pub reserve_output: usize,
}

impl ContextBudget {
    /// Резерв под ответ по умолчанию (примерно один развёрнутый ответ модели).
    pub const DEFAULT_RESERVE_OUTPUT: usize = 1024;

    /// Консервативный дефолт контекстного окна (токены), если `context_window` не задан в конфиге
    /// (INFER-CFG). Безопасный ПОЛ для всех развёрнутых/целевых моделей (текущий llama.cpp = 32K).
    /// Старый дефолт был 8192 — он голодил RAG/память на 32K/256K-моделях. 256K НЕ хардкодим — для
    /// больших окон значение задаётся явно в `.nexus/local.json` (`ai.chat.context_window`).
    pub const DEFAULT_CONTEXT_WINDOW: usize = 32768;

    /// Бюджет из контекстного окна модели; если окно не задано в конфиге — консервативный дефолт
    /// [`Self::DEFAULT_CONTEXT_WINDOW`] (32K) с предупреждением в лог. Резерв под ответ — дефолтный
    /// ([`Self::DEFAULT_RESERVE_OUTPUT`]); конфигурируемый резерв — через [`Self::with_reserve`].
    pub fn from_context_window(context_window: Option<usize>) -> Self {
        Self::with_reserve(context_window, Self::DEFAULT_RESERVE_OUTPUT)
    }

    /// Как [`Self::from_context_window`], но с ЯВНЫМ резервом под ответ (INFER-CFG:
    /// `ChatConfig::reserve_output_tokens()`). `None` окно → дефолт 32K + `warn!`.
    pub fn with_reserve(context_window: Option<usize>, reserve_output: usize) -> Self {
        let context_window = context_window.unwrap_or_else(|| {
            tracing::warn!(
                default_window = Self::DEFAULT_CONTEXT_WINDOW,
                "context_window не задан в .nexus/local.json (ai.chat.context_window); беру \
                 консервативные 32K — для 256K-моделей (напр. Qwen3.6-27B на vLLM) задай явно"
            );
            Self::DEFAULT_CONTEXT_WINDOW
        });
        Self {
            context_window,
            reserve_output,
        }
    }

    /// Доступный бюджет под ВХОДНЫЕ сообщения (окно минус резерв под ответ), не уходит ниже 0.
    pub fn input_budget(&self) -> usize {
        self.context_window.saturating_sub(self.reserve_output)
    }

    /// Пер-сообщенческий оверхед обёртки ChatML (`<|im_start|>role\n … <|im_end|>\n`). Реально ~6–8
    /// токенов; берём 8 консервативно — бюджет должен скорее переоценить, чем переполнить окно.
    /// `pub(crate)`: ЕДИНЫЙ источник константы — цикл агента (`agent::runner::count_used`) считает
    /// `used` по той же формуле, не дублируя число (иначе оценки бюджета разъехались бы).
    pub(crate) const PER_MESSAGE_OVERHEAD: usize = 8;

    /// Стоимость одного сообщения: токены контента + пер-сообщенческий оверхед ChatML. `pub(crate)`,
    /// чтобы цикл агента считал `used` ровно той же формулой, что `fit` (одно место для cost-математики).
    pub(crate) fn message_cost(tk: &dyn Tokenizer, m: &ChatMessage) -> usize {
        tk.count(&m.content) + Self::PER_MESSAGE_OVERHEAD
    }

    /// Укладывает сообщения в [`input_budget`], сохраняя ВСЕ system-сообщения и максимум самых
    /// СВЕЖИХ не-system. Порядок результата = порядок входа (system на своих местах, хвост диалога
    /// сохранён, выкинуты самые старые середины-реплики).
    ///
    /// # Контракт вызывающего (важно)
    /// system-сообщения здесь НИКОГДА не режутся. Если ОДНИ system превышают [`input_budget`],
    /// результат всё равно содержит все system, и его сумма токенов + `reserve_output` может
    /// ПРЕВЫСИТЬ `context_window`. Это намеренно (инструкции/память/RAG не теряем молча). Поэтому
    /// вызывающий ОБЯЗАН проверить `sum(результат) + reserve_output <= context_window` и
    /// деградировать явно (короче system / меньше RAG-контекста). При переполнении одними system
    /// пишется `warn!` — молчаливого оверфлоу нет.
    pub fn fit(&self, tk: &dyn Tokenizer, messages: &[ChatMessage]) -> Vec<ChatMessage> {
        let budget = self.input_budget();

        // 1) system-сообщения сохраняются всегда (инструкции/память/RAG-контекст не режем здесь).
        let mut used: usize = messages
            .iter()
            .filter(|m| m.role == "system")
            .map(|m| Self::message_cost(tk, m))
            .sum();
        if used > budget {
            tracing::warn!(
                system_tokens = used,
                input_budget = budget,
                "ContextBudget::fit: одни system-сообщения превышают бюджет — не режем их, но \
                 результат может превысить context_window; вызывающий должен деградировать явно"
            );
        }

        // 2) Не-system добавляем С КОНЦА (самые свежие приоритетнее), пока влезает.
        //    Индексы выбранных не-system запоминаем, чтобы восстановить исходный порядок.
        let mut keep_nonsystem: Vec<usize> = Vec::new();
        for (i, m) in messages.iter().enumerate().rev() {
            if m.role == "system" {
                continue;
            }
            let cost = Self::message_cost(tk, m);
            if used + cost <= budget {
                used += cost;
                keep_nonsystem.push(i);
            }
            // Не break: более старое короткое сообщение могло бы влезть, но это нарушило бы
            // непрерывность хвоста диалога. Останавливаемся на первом, что не влез (рекенси-окно).
            else {
                break;
            }
        }

        // 3) Сборка в ИСХОДНОМ порядке: system на местах + выбранные свежие не-system.
        let keep_set: std::collections::HashSet<usize> = keep_nonsystem.into_iter().collect();
        messages
            .iter()
            .enumerate()
            .filter(|(i, m)| m.role == "system" || keep_set.contains(i))
            .map(|(_, m)| m.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Golden-строки и их ТОЧНЫЕ счётчики на ЗАДЕПЛОЕННОЙ модели (Qwen3.6-27B :8080), снятые с
    /// `POST /tokenize` живого сервера. Это и есть ГЕЙТ: встроенный токенайзер обязан давать ровно
    /// эти числа. При расхождении — ассет ≠ задеплоенная модель (НЕ править goldens, чинить ассет).
    /// Те же строки гоняет live-кросс-чек `crate::eval::live_tests::live_tokenizer_matches_server`.
    pub(crate) const GOLDEN: &[(&str, usize)] = &[
        (
            "The quick brown fox jumps over the lazy dog. Knowledge management is the second brain.",
            17,
        ),
        (
            "Векторный поиск по заметкам с переранжированием — это основа второго мозга на каждый день.",
            24,
        ),
        (
            "fn main() { let xs: Vec<i32> = (0..10).filter(|n| n % 2 == 0).collect(); println!(\"{:?}\", xs); }",
            40,
        ),
        (
            "Agent обходит vault и пишет файлы: создать заметку 'проект.md' с тегами #nexus #агент.",
            27,
        ),
    ];

    /// ГЕЙТ P0-c: встроенный токенайзер == задеплоенная модель на всех golden (офлайн, без сети).
    #[test]
    fn embedded_matches_deployed_model_counts() {
        let tk = QwenTokenizer::embedded();
        assert!(
            embedded_tokenizer().is_some(),
            "встроенный токенайзер не загрузился — ассет повреждён/невалиден"
        );
        for (text, expected) in GOLDEN {
            let got = tk.count(text);
            assert_eq!(
                got, *expected,
                "встроенный токенайзер ≠ задеплоенная модель: {got} ≠ {expected} для {text:?}\n\
                 (НЕ править goldens — это числа /tokenize живого сервера; чинить ассет)"
            );
        }
    }

    /// Кириллица доказывает РЕАЛЬНЫЙ токенайзер: BPE-счёт (24) сильно > числа слов (~13).
    #[test]
    fn cyrillic_count_exceeds_word_count() {
        let tk = QwenTokenizer::embedded();
        let ru = GOLDEN[1].0;
        let bpe = tk.count(ru);
        let words = ru.split_whitespace().count();
        assert!(
            bpe >= 2 * words || bpe > words + 8,
            "BPE-счёт кириллицы ({bpe}) должен заметно превышать число слов ({words}) — иначе \
             работает не реальный токенайзер, а словесная эвристика"
        );
        assert_eq!(bpe, 24, "RU golden — ровно 24 BPE-токена");
    }

    /// Fallback при битом пути: `from_file` на несуществующем файле → встроенный (не паника),
    /// счёт совпадает с встроенным (т.к. встроенный загрузился).
    #[test]
    fn from_file_bad_path_falls_back_to_embedded() {
        let tk = QwenTokenizer::from_file(std::path::Path::new("/nonexistent/tokenizer.json"));
        // Встроенный загружен → точные счётчики сохраняются.
        for (text, expected) in GOLDEN {
            assert_eq!(
                tk.count(text),
                *expected,
                "fallback на встроенный, {text:?}"
            );
        }
    }

    /// Эвристика-fallback: разумный консервативный счёт, без паники, кириллица > латиница на симв.
    #[test]
    fn heuristic_is_conservative_and_never_panics() {
        let h = HeuristicTokenizer;
        assert_eq!(h.count(""), 0);
        assert_eq!(h.count("   \n\t "), 0);
        assert_eq!(h.count("a"), 1);
        // Латиница: 16 непробельных символов / 4 ≈ 4.
        assert_eq!(h.count("abcd efgh ijkl mnop"), 4);
        // Кириллица плотнее (÷3): тот же объём символов → больше токенов.
        let ru = "Векторный поиск по заметкам";
        let latin_like = "a".repeat(ru.chars().filter(|c| !c.is_whitespace()).count());
        assert!(
            h.count(ru) > h.count(&latin_like),
            "кириллица должна оцениваться дороже латиницы при равном числе символов"
        );
        // CJK ≈ 1 символ/токен.
        assert!(h.count("漢字漢字漢字漢字漢字") >= 7);
        // Никогда не панкует на странном вводе.
        let _ = h.count("🦀🦀🦀 mixed эмодзи 漢字 123 !!!");
    }

    /// `ContextBudget::fit`: режет до бюджета, сохраняя system + самые свежие реплики.
    #[test]
    fn fit_keeps_system_and_recent() {
        let tk = HeuristicTokenizer; // детерминированно, без зависимости от ассета
                                     // input_budget = 145-100 = 45 токенов. Стоимости (эвристика): system=14, каждое user≈12-13.
                                     // Влезает system(14)+newest(13)+newer(13)=40; middle(13)→53>45 → стоп. Старые отброшены.
        let budget = ContextBudget {
            context_window: 145,
            reserve_output: 100,
        };
        let sys = ChatMessage::system("you are a helpful second brain assistant tool");
        let m = |s: &str| ChatMessage::user(s.to_string());
        let messages = vec![
            sys.clone(),
            m("oldest message number one here padding"),
            m("older message number two here padding"),
            m("middle message number three padding text"),
            m("newer message number four here padding"),
            m("newest message number five here padding"),
        ];
        let fitted = budget.fit(&tk, &messages);
        // system всегда сохранён и на первом месте.
        assert_eq!(fitted[0].role, "system");
        assert!(fitted.iter().filter(|x| x.role == "system").count() == 1);
        // Самое свежее сохранено, самое старое отброшено.
        assert!(fitted.iter().any(|x| x.content.contains("newest")));
        assert!(!fitted.iter().any(|x| x.content.contains("oldest")));
        // Влезли не все (бюджет жмёт) и порядок исходный (newer идёт перед newest).
        assert!(fitted.len() < messages.len());
        let order: Vec<&str> = fitted.iter().map(|x| x.content.as_str()).collect();
        let pos_newer = order.iter().position(|s| s.contains("newer"));
        let pos_newest = order.iter().position(|s| s.contains("newest"));
        if let (Some(a), Some(b)) = (pos_newer, pos_newest) {
            assert!(a < b, "исходный порядок диалога должен сохраняться");
        }
    }

    /// `fit`: всё влезает → ничего не отброшено, порядок сохранён.
    #[test]
    fn fit_keeps_all_when_under_budget() {
        let tk = HeuristicTokenizer;
        let budget = ContextBudget {
            context_window: 100_000,
            reserve_output: 1024,
        };
        let messages = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("hi"),
            ChatMessage::assistant("hello"),
            ChatMessage::user("bye"),
        ];
        let fitted = budget.fit(&tk, &messages);
        assert_eq!(fitted.len(), 4);
        assert_eq!(fitted[0].content, "sys");
        assert_eq!(fitted[3].content, "bye");
    }

    /// `fit`: даже если system не влезает в бюджет — system НЕ режется (не теряем инструкции).
    #[test]
    fn fit_never_drops_system_even_if_over_budget() {
        let tk = HeuristicTokenizer;
        let budget = ContextBudget {
            context_window: 5,
            reserve_output: 4,
        }; // input_budget = 1, ничтожный
        let messages = vec![
            ChatMessage::system("a long system prompt that exceeds the tiny budget on its own"),
            ChatMessage::user("user message"),
        ];
        let fitted = budget.fit(&tk, &messages);
        assert_eq!(fitted.len(), 1);
        assert_eq!(fitted[0].role, "system");
    }

    /// Контракт (by design): когда ОДНИ system превышают `input_budget`, `fit` всё равно их вернёт,
    /// и сумма токенов результата ПРЕВЫШАЕТ бюджет — вызывающий обязан сам проверять окно (см.
    /// docstring `fit` + warn!). Фиксируем поведение, чтобы рефактор не «починил» его молча.
    #[test]
    fn fit_system_overflow_exceeds_budget_by_design() {
        let tk = HeuristicTokenizer;
        let budget = ContextBudget {
            context_window: 60,
            reserve_output: 20,
        }; // input_budget = 40
        let big_system = "очень длинная системная инструкция ".repeat(20); // заведомо > 40 токенов
        let messages = vec![ChatMessage::system(big_system), ChatMessage::user("вопрос")];
        let fitted = budget.fit(&tk, &messages);
        assert!(fitted.iter().any(|m| m.role == "system"), "system сохранён");
        let total: usize = fitted
            .iter()
            .map(|m| ContextBudget::message_cost(&tk, m))
            .sum();
        assert!(
            total > budget.input_budget(),
            "by design: одни system превышают бюджет ({total} > {})",
            budget.input_budget()
        );
    }

    /// `from_context_window`: None → консервативный дефолт (INFER-CFG: 32K, не 8192), Some → ровно
    /// из конфига; 256K-окно уважается; with_reserve прокидывает конфигурируемый резерв.
    #[test]
    fn budget_reads_window_from_config() {
        assert_eq!(
            ContextBudget::from_context_window(Some(32768)).context_window,
            32768
        );
        // INFER-CFG: дефолт окна поднят 8192 → 32768 (безопасный пол; голод RAG на больших моделях).
        assert_eq!(
            ContextBudget::from_context_window(None).context_window,
            32768
        );
        assert_eq!(
            ContextBudget::from_context_window(None).context_window,
            ContextBudget::DEFAULT_CONTEXT_WINDOW
        );
        // 256K-модель (Qwen3.6-27B на vLLM): значение из конфига уважается (256K не хардкодим).
        assert_eq!(
            ContextBudget::from_context_window(Some(262144)).context_window,
            262144
        );
        assert_eq!(
            ContextBudget::from_context_window(Some(100)).input_budget(),
            100usize.saturating_sub(ContextBudget::DEFAULT_RESERVE_OUTPUT)
        );
        // with_reserve: явный резерв уважается (None окно → дефолт 32K, warn не валит).
        let b = ContextBudget::with_reserve(None, 4096);
        assert_eq!(b.context_window, ContextBudget::DEFAULT_CONTEXT_WINDOW);
        assert_eq!(b.reserve_output, 4096);
        assert_eq!(
            ContextBudget::with_reserve(Some(262144), 2048).input_budget(),
            262144 - 2048
        );
    }

    /// Эвристика дешевле точного, но не абсурдно: на golden-строках в пределах разумного коридора.
    #[test]
    fn heuristic_in_reasonable_range_of_real() {
        let real = QwenTokenizer::embedded();
        let h = HeuristicTokenizer;
        for (text, _) in GOLDEN {
            let r = real.count(text);
            let e = h.count(text);
            // Консервативная: не сильно НИЖЕ реального (это и есть риск переполнения окна).
            // Допускаем коридор [0.5×, 2×] реального — груба, но без катастроф.
            assert!(
                e * 2 >= r && e <= r * 2 + 4,
                "эвристика {e} вне разумного коридора реального {r} для {text:?}"
            );
        }
    }
}
