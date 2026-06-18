//! Команды HOME-дашборда. H1: статические/динамические виджеты (stats/recent/goals) одним запросом.
//! H2: кэш LLM-виджетов — `get_widget` (мгновенно из кэша) + `refresh_widget` (ручной refresh поверх
//! планировщика ADR-007). См. `docs/dev/HOME_BACKEND_PLAN.md`.

use tauri::State;

use crate::error::AppResult;
use crate::home::activity::{self, ActivityData};
use crate::home::stale::{self, StaleNote};
use crate::home::widgets::{self, Widget};
use crate::home::{self, HomeData};
use crate::scheduler;
use crate::state::AppState;

/// Длина сниппета «Продолжить» (символы).
const CONTINUE_SNIPPET_CHARS: usize = 180;

/// Данные HOME для статических/динамических зон (без LLM). Без открытого vault — ошибка.
#[tauri::command]
pub async fn get_home_data(state: State<'_, AppState>) -> AppResult<HomeData> {
    let reader = state.vault().await?.db.reader().clone();
    Ok(home::home_data(&reader).await?)
}

/// H6 (DP-1): зона «Активность» — heatmap правок, серия дней, сироты, «Продолжить» со сниппетом.
/// `tz_offset_min` — как `Date.getTimezoneOffset()` (минуты западнее UTC): локальные дни юзера.
#[tauri::command]
pub async fn get_home_activity(
    state: State<'_, AppState>,
    tz_offset_min: i32,
) -> AppResult<ActivityData> {
    let (root, reader) = {
        let ctx = state.vault().await?;
        (ctx.root.clone(), ctx.db.reader().clone())
    };
    let mut data = activity::activity_data(
        &reader,
        crate::scheduler::now_secs(),
        i64::from(tz_offset_min),
    )
    .await?;
    // Сниппет «Продолжить» — чтение головы файла с диска (путь из БД, внутри vault).
    if let Some(cont) = data.continue_note.as_mut() {
        if let Ok(body) = std::fs::read_to_string(root.join(&cont.path)) {
            cont.snippet = activity::continue_snippet(&body, CONTINUE_SNIPPET_CHARS);
        }
    }
    Ok(data)
}

/// Кэшированный LLM-виджет по ключу (или `null`, если ещё не генерировался). Мгновенно — НЕ ждёт LLM
/// (генерация идёт фоном; готовность прилетает событием `home:widget-updated`). Без vault — `null`
/// (виджет просто не покажется).
#[tauri::command]
pub async fn get_widget(state: State<'_, AppState>, key: String) -> AppResult<Option<Widget>> {
    let reader = {
        let g = state.vault.read().await;
        match g.as_ref() {
            Some(ctx) => ctx.db.reader().clone(),
            None => return Ok(None),
        }
    };
    Ok(widgets::get(&reader, &key).await?)
}

/// Ручной refresh виджета (режим manual): ставит фоновую генерацию в очередь. Ключ должен быть
/// зарегистрированным виджетом (иначе понятная ошибка вместо тихого dead-letter). Дедуп: если генерация
/// уже готова к запуску/выполняется — повторный клик no-op. Результат — событием `home:widget-updated`.
#[tauri::command]
pub async fn refresh_widget(state: State<'_, AppState>, key: String) -> AppResult<()> {
    let (writer, reader, kind) = {
        let ctx = state.vault().await?;
        (
            ctx.db.writer().clone(),
            ctx.db.reader().clone(),
            ctx.widgets.kind_for(&key).map(str::to_string),
        )
    };
    let Some(kind) = kind else {
        return Err(format!("неизвестный HOME-виджет: {key}").into());
    };
    // Тоггл «Инсайты» OFF → ручной refresh инсайт-виджета no-op (как сид/recurring гейтятся в vault.rs).
    if matches!(
        key.as_str(),
        crate::home::insights::KEY_OPEN_QUESTIONS | crate::home::insights::KEY_CONTEXT_DRIFT
    ) && !crate::home::insights::insights_enabled(&reader).await
    {
        return Ok(());
    }
    if scheduler::has_ready_job(&reader, &kind, scheduler::now_secs()).await? {
        return Ok(()); // уже в очереди/выполняется — дедуп
    }
    scheduler::enqueue(&writer, &kind, "", 0, 2).await?;
    Ok(())
}

/// «Stale radar» (H4): ранжированный список устаревших заметок. Слой 1 (скоринг) считается на лету;
/// слой 2 (LLM-причина/действие/подсказка) приходит из кэша, если заметку уже обогащали. Мгновенно,
/// on-open. Без открытого vault — пусто (панель просто не покажет).
#[tauri::command]
pub async fn get_stale_radar(state: State<'_, AppState>) -> AppResult<Vec<StaleNote>> {
    let reader = {
        let g = state.vault.read().await;
        match g.as_ref() {
            Some(ctx) => ctx.db.reader().clone(),
            None => return Ok(Vec::new()),
        }
    };
    Ok(stale::scan(&reader, scheduler::now_secs()).await?)
}

/// Ручной запуск LLM-обогащения «Stale radar» (слой 2, manual): топ-N устаревших → причина/действие/
/// подсказка, кэш 24ч. Требует chat (LLM); дедуп активной джобы. Результат — событие `home:widget-updated`.
#[tauri::command]
pub async fn refresh_stale_radar(state: State<'_, AppState>) -> AppResult<()> {
    let (writer, reader, has_chat) = {
        let ctx = state.vault().await?;
        (
            ctx.db.writer().clone(),
            ctx.db.reader().clone(),
            ctx.ai.chat.is_some(),
        )
    };
    if !has_chat {
        return Err("chat (LLM) не сконфигурирован — настройте в «AI / Модели»".into());
    }
    // Stale radar — часть «Инсайтов»: при выключенном тогле ручной refresh no-op.
    if !crate::home::insights::insights_enabled(&reader).await {
        return Ok(());
    }
    if scheduler::has_ready_job(&reader, stale::KIND_STALE, scheduler::now_secs()).await? {
        return Ok(()); // уже в очереди/выполняется — дедуп
    }
    scheduler::enqueue(&writer, stale::KIND_STALE, "", 0, 2).await?;
    Ok(())
}

/// Текущее состояние тоггла «Инсайты» (проактивные ИИ-виджеты Home: открытые вопросы + дрейф контекста +
/// stale-radar). Persisted, дефолт OFF.
#[tauri::command]
pub async fn insights_get_enabled(state: State<'_, AppState>) -> AppResult<bool> {
    let reader = state.vault().await?.db.reader().clone();
    Ok(home::insights::insights_enabled(&reader).await)
}

/// Переключить «Инсайты». Persist `insights.enabled` + при ВКЛЮЧЕНИИ — enqueue kick для каждого
/// доступного виджета (зеркало `episode_set_enabled`/`contradictions_set_enabled`: сид/recurring гейтятся
/// флагом и регистрируются лишь на открытии vault → без kick включение в работающем приложении не
/// сгенерирует виджеты до перезапуска). Дедуп `has_ready_job`. Хендлеры зарегистрированы всегда.
#[tauri::command]
pub async fn insights_set_enabled(state: State<'_, AppState>, on: bool) -> AppResult<()> {
    let (writer, reader, has_util, has_fast) = {
        let ctx = state.vault().await?;
        (
            ctx.db.writer().clone(),
            ctx.db.reader().clone(),
            ctx.ai.chat_util.is_some(),
            ctx.ai.chat_fast.is_some(),
        )
    };
    home::insights::set_insights_enabled(&writer, on).await?;
    if on {
        let now = scheduler::now_secs();
        // open_questions + stale → chat_util; context_drift → chat_fast.
        let mut kicks: Vec<String> = Vec::new();
        if has_util {
            kicks.push(home::widgets::widget_kind(
                home::insights::KEY_OPEN_QUESTIONS,
            ));
            kicks.push(stale::KIND_STALE.to_string());
        }
        if has_fast {
            kicks.push(home::widgets::widget_kind(
                home::insights::KEY_CONTEXT_DRIFT,
            ));
        }
        for kind in kicks {
            if !scheduler::has_ready_job(&reader, &kind, now).await? {
                let _ = scheduler::enqueue(&writer, &kind, "", 0, 2).await;
            }
        }
    }
    Ok(())
}
