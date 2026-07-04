import { Channel, invoke } from '@tauri-apps/api/core';
import * as mockAgent from '../../mock/agent';
import * as mockSettings from '../../mock/settings';
import { bridge, isTauri } from '../bridge';
import type {
  AgentApprovalDecision,
  AgentAutonomy,
  AgentConnectionDto,
  AgentHistoryMsg,
  AgentSessionData,
  AgentSessionInfo,
  AgentStreamEvent,
  SkillList,
} from './types';

/**
 * Agent-домен (F-2c): запуск/контроль прогона агента + стрим событий (UI-1), история переписок
 * (W-38), skills-панель (W-10), подключение агента (CONN-4/ACP — настройки+пробы). Request/
 * response-вызовы — через `bridge` (Tauri ↔ мок `lib/mock/*`); потребители ходят сюда по-прежнему
 * через `tauriApi.agent` / `tauriApi.settings.setAgentConnection`/`testAgentConnection`
 * (barrel-реэкспорт в `lib/tauri-api.ts`).
 */

/**
 * Агент (UI-1): запуск/контроль прогона + стрим событий. `run` создаёт `Channel<AgentStreamEvent>`,
 * подвешивает `onEvent` на `channel.onmessage` (как `chat.streamRag`), зовёт `agent_run` и резолвится
 * `run_id`. Вне Tauri — мок-стрим (`lib/mock/agent`), ЗЕРКАЛЯЩИЙ контракт (те же формы/порядок событий).
 */
export const agent = {
  /**
   * Запускает прогон: стримит события в `onEvent`, возвращает Promise<run_id>. Стрим асинхронный —
   * run_id приходит сразу, события докапываются по ходу. Ошибку `agent_run` (нет vault и т.п.)
   * прокидываем в `onEvent` как `error`-событие И реджектим Promise (стор покажет ошибку).
   *
   * Честное bridge-исключение (см. шапку `../bridge.ts`): стрим-команда с `Channel` (как
   * `chat.streamRag`) — канал + `onmessage` + Promise<run_id>, это не request/response-форма
   * `bridge`, поэтому остаётся прямым `invoke`.
   */
  run: (
    task: string,
    autonomy: AgentAutonomy,
    onEvent: (event: AgentStreamEvent) => void,
    // W-4: история прошлых ходов сессии (мультитёрн) — чтобы follow-up продолжал работу прошлого
    // хода и снова предлагал правки через гейт. Пусто для первого хода.
    history: AgentHistoryMsg[] = [],
    // W-38: id переписки (группировка ходов для истории). Опционален для обратной совместимости.
    sessionId?: string,
  ): Promise<number> => {
    if (!isTauri()) return mockAgent.run(task, autonomy, onEvent, history, sessionId);
    const channel = new Channel<AgentStreamEvent>();
    channel.onmessage = onEvent;
    return invoke<number>('agent_run', { task, autonomy, history, sessionId, channel }).catch(
      (e: unknown) => {
        onEvent({ type: 'error', message: String(e) });
        throw e;
      },
    );
  },
  /** W-38: история переписок агента (левый сайдбар). list — сводки, load — ходы переписки. */
  sessions: {
    list: (): Promise<AgentSessionInfo[]> =>
      bridge<AgentSessionInfo[]>('agent_sessions_list', undefined, () => mockAgent.sessionsList()),
    load: (sessionId: string): Promise<AgentSessionData> =>
      bridge<AgentSessionData>('agent_session_load', { sessionId }, () =>
        mockAgent.sessionLoad(sessionId),
      ),
  },
  /** Кормит UI-DecisionSource прогона решениями (Confirm-тир аппрув/реджект). */
  approve: (runId: number, decisions: AgentApprovalDecision[]): Promise<void> =>
    bridge<void>('agent_approve', { runId, decisions }, () => mockAgent.approve(runId, decisions)),
  /** Пауза прогона (AGENT-5 kill-switch). */
  pause: (runId: number): Promise<void> =>
    bridge<void>('agent_pause', { runId }, () => mockAgent.pause(runId)),
  /** Снять паузу прогона. */
  resume: (runId: number): Promise<void> =>
    bridge<void>('agent_resume', { runId }, () => mockAgent.resume(runId)),
  /** Кооперативно отменить прогон. */
  cancel: (runId: number): Promise<void> =>
    bridge<void>('agent_cancel', { runId }, () => mockAgent.cancel(runId)),
  /** Откат применённых действий прогона (AGENT-4) → число откаченных. */
  undo: (runId: number): Promise<number> =>
    bridge<number>('agent_undo', { runId }, () => mockAgent.undo(runId)),
  /** W-10: список авто-навыков агента (read-only) + состояние самообучения. */
  listSkills: (): Promise<SkillList> =>
    bridge<SkillList>('agent_list_skills', undefined, () => mockAgent.listSkills()),
  /** W-10: закрепить/открепить навык (no-op на vendor/user). */
  setSkillPinned: (name: string, pinned: boolean): Promise<boolean> =>
    bridge<boolean>('agent_skill_set_pinned', { name, pinned }, () =>
      mockAgent.setSkillPinned(name, pinned),
    ),
  /** W-10: архивировать/разархивировать навык (обратимо; НЕ «выключить»). */
  setSkillArchived: (name: string, archived: boolean): Promise<boolean> =>
    bridge<boolean>('agent_skill_set_archived', { name, archived }, () =>
      mockAgent.setSkillArchived(name, archived),
    ),
};

/** Подключение агента (CONN-4/ACP-1b/ACP-REMOTE-SSH): персист режима + проба связи. Мок живёт в
 *  `mock/settings.ts` (делит in-memory конфиг с getAiConfig — connection часть `ai.*`). В барреле
 *  реэкспортируется как `tauriApi.settings.setAgentConnection`/`testAgentConnection`. */
export const agentConnection = {
  /** CONN-4/ACP-1b/ACP-REMOTE-SSH: персистит режим подключения агента (`ai.connection`) + немедленно
   *  свопает бэкенд. `mode` нормализуется (мусор → embedded); `null`-поля → бэк не трогает соответствующее
   *  поле. Для acp-ssh передаются transport/host/key/remoteCommand; для acp-local — acpCommand. Возвращает
   *  записанное. */
  set: (
    mode: 'embedded' | 'local' | 'remote' | 'acp',
    socket: string | null,
    acpCommand: string | null = null,
    acpCwd: string | null = null,
    acpTransport: string | null = null,
    acpSshHost: string | null = null,
    acpSshKey: string | null = null,
    acpRemoteCommand: string | null = null,
  ): Promise<AgentConnectionDto> =>
    bridge<AgentConnectionDto>(
      'set_agent_connection',
      {
        mode,
        socket,
        acpCommand,
        acpCwd,
        acpTransport,
        acpSshHost,
        acpSshKey,
        acpRemoteCommand,
      },
      () =>
        mockSettings.setAgentConnection(
          mode,
          socket,
          acpCommand,
          acpCwd,
          acpTransport,
          acpSshHost,
          acpSshKey,
          acpRemoteCommand,
        ),
    ),

  /** CONN-4/ACP-1b: проверка связи (local: AF_UNIX handshake; acp: spawn+initialize). Резолвится строкой
   *  версии протокола; throw = недоступен / неверный режим. */
  test: (): Promise<string> =>
    bridge<string>('test_agent_connection', undefined, () => mockSettings.testAgentConnection()),
};
