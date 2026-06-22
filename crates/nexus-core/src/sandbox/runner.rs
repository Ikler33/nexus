//! `SandboxRunner` — host-side оркестратор песочного прогона (SANDBOX-4b-2b, спека §2/§5).
//!
//! Зеркало [`super::child`] на ХОСТЕ: биндит 3 AF_UNIX-сокета в per-run каталоге
//! ([`SandboxConfig::host_run_dir`], НЕ под `:ro`-vault), спавнит хардненный `podman run`
//! ([`sandbox_run_plan_with_cmd`] + `--sandbox-child …`) и ОБСЛУЖИВАЕТ каждый сокет РЕАЛЬНЫМ backend'ом:
//! - **egress.sock** — [`GuardedProxy`] (→ `GuardedClient`, единственный сетевой путь);
//! - **act.sock** — [`HostActServer`] (→ `dispatch_action`, authoritative-гейт host-side);
//! - **event.sock** — [`EventForwardServer`] (релей событий в исходящий транспорт коннектора/десктоп).
//!
//! Контейнер (`--network=none`) коннектится к ним как КЛИЕНТ. Host держит authoritative-решения; контейнер
//! не пишет локально и не имеет иного сетевого пути. Lifecycle: `run()` ждёт выхода контейнера (агент-loop
//! завершился → процесс вышел → `--rm` снёс контейнер). Отмена — `podman kill <container_name>` (имя в
//! [`super::SandboxPlan::container_name`]); проводка к `agent/cancel` — последующий срез.
//!
//! Tier-1-тестируемы serve-хелперы (через `ChannelTransport`); полный `run()` — Tier-2 (нужен Podman +
//! образ; podman-gated интеграционный тест, живёт на .28).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::net::{UnixListener, UnixStream};

use crate::agent::connect::{
    harden_socket_perms, peer_uid, prepare_socket_path, AfUnixTransport, RpcMessage, Transport,
};

use super::act::{ActuatorBackend, HostActServer};
use super::event::EventForwardServer;
use super::proxy::{EgressBackend, GuardedProxy};
use super::{sandbox_run_plan_with_cmd, SandboxConfig, SOCKET_ACT, SOCKET_EGRESS, SOCKET_EVENT};

/// Чистое сопоставление peer-uid с ожидаемым (без I/O — тестируемо на любой ОС). **Fail-closed:**
/// неизвестный ожидаемый (`run_as` не выставлен/невалиден) ИЛИ нечитаемый фактический uid (`SO_PEERCRED`
/// не прочёлся) → НЕ авторизован. Авторизуем ТОЛЬКО при достоверном равенстве обоих.
fn uid_matches(expected: Option<u32>, actual: Option<u32>) -> bool {
    matches!((expected, actual), (Some(e), Some(a)) if e == a)
}

/// Авторизует принятое на per-run сокете соединение по `SO_PEERCRED` (спека §4.3 инвариант 6 / §10.1 T8):
/// валидно ТОЛЬКО если peer бежит под `expected_uid` (= host-видимый uid контейнера, см. `run()`).
/// Defense-in-depth ПОВЕРХ 0600-сокета + 0700-каталога — ядро-достоверный uid, который клиент не подделает.
/// Fail-closed (см. [`uid_matches`] и [`peer_uid`]). **Тем же гейтом ОБЯЗАН оборачиваться будущий
/// `serve_exec` (exec.sock).**
fn peer_authorized(stream: &UnixStream, expected_uid: Option<u32>) -> bool {
    uid_matches(expected_uid, peer_uid(stream))
}

/// Обслуживает ОДНО соединение egress.sock: фреймит request → [`GuardedProxy::handle`] → response, до
/// закрытия транспорта. Контейнер открывает ровно одно соединение на сокет.
pub async fn serve_egress<T: Transport, B: EgressBackend>(transport: T, proxy: &GuardedProxy<B>) {
    while let Some(msg) = transport.recv().await {
        if let RpcMessage::Request { id, method, params } = msg {
            let result = proxy.handle(&method, params).await;
            if transport
                .send(RpcMessage::Response { id, result })
                .await
                .is_err()
            {
                break;
            }
        }
        // Не-Request на egress.sock не ожидаются — игнор (контейнер только запрашивает).
    }
}

/// Обслуживает ОДНО соединение act.sock: request → [`HostActServer::handle`] → response, до закрытия.
pub async fn serve_act<T: Transport, B: ActuatorBackend>(transport: T, server: &HostActServer<B>) {
    while let Some(msg) = transport.recv().await {
        if let RpcMessage::Request { id, method, params } = msg {
            let result = server.handle(&method, params).await;
            if transport
                .send(RpcMessage::Response { id, result })
                .await
                .is_err()
            {
                break;
            }
        }
    }
}

/// Аргументы команды контейнера для `nexus-agentd --sandbox-child` (после образа в `podman run`).
/// Передаются ARGV (не шелл) — `task` с пробелами/спецсимволами безопасен. Сокеты контейнер берёт по
/// ФИКСИРОВАННЫМ путям (`CONTAINER_RUN_DIR/{egress,act,event}.sock`) — в argv их нет.
pub struct SandboxChildArgs {
    pub run_id: String,
    pub base_url: String,
    pub model: String,
    pub context_window: usize,
    pub task: String,
}

impl SandboxChildArgs {
    /// Позиционный argv: `--sandbox-child <run_id> <base_url> <model> <ctx_window> <task>`.
    pub fn to_argv(&self) -> Vec<String> {
        vec![
            "--sandbox-child".into(),
            self.run_id.clone(),
            self.base_url.clone(),
            self.model.clone(),
            self.context_window.to_string(),
            self.task.clone(),
        ]
    }
}

/// Host-оркестратор: держит [`SandboxConfig`] (план podman + per-run каталог сокетов).
pub struct SandboxRunner {
    config: SandboxConfig,
}

impl SandboxRunner {
    pub fn new(config: SandboxConfig) -> Self {
        Self { config }
    }

    /// Путь сокета на ХОСТЕ в per-run каталоге.
    fn socket_path(&self, name: &str) -> PathBuf {
        self.config.host_run_dir.join(name)
    }

    /// Bind ОДНОГО сокета с хардненингом (ЕДИНАЯ реализация коннектора): `prepare_socket_path`
    /// (отказ удалять НЕ-сокет — не трём чужой файл) → `bind` → `harden_socket_perms` (0600 — спека §4.2/
    /// §4.3: per-run сокеты owner-only; egress.sock = guarded-эгресс, act.sock = host-гейт записи).
    fn bind_hardened(path: &Path) -> Result<UnixListener, String> {
        prepare_socket_path(path)
            .map_err(|e| format!("подготовить путь сокета {}: {e}", path.display()))?;
        let listener =
            UnixListener::bind(path).map_err(|e| format!("bind {}: {e}", path.display()))?;
        harden_socket_perms(path); // 0600 СРАЗУ после bind (как serve_unix_at)
        Ok(listener)
    }

    /// Гонит песочный прогон end-to-end: каталог сокетов → bind 3 сокета → spawn serve-таски → spawn
    /// `podman run --sandbox-child …` → ждать выхода контейнера → teardown. Возвращает код выхода
    /// контейнера. **Tier-2** (нужен Podman + образ; на хосте без Podman вернёт ошибку spawn).
    pub async fn run<Eb, Ab>(
        &self,
        child: SandboxChildArgs,
        egress_proxy: GuardedProxy<Eb>,
        act_server: HostActServer<Ab>,
        event_out: Arc<dyn Transport>,
    ) -> Result<std::process::ExitStatus, String>
    where
        Eb: EgressBackend + 'static,
        Ab: ActuatorBackend + 'static,
    {
        let dir = self.config.host_run_dir.clone();
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("создать каталог сокетов {}: {e}", dir.display()))?;

        // КРИТИЧНО (live-bug на .28): процесс контейнера ДОЛЖЕН бежать под host-uid, иначе
        // непривилегированный USER образа (uid 10001) + `--userns=keep-id` НЕ откроет host-owned
        // 0600-сокеты/`:ro`-vault (EACCES). Берём uid:gid из ТОЛЬКО ЧТО созданного нами каталога (его
        // владелец = наш процесс) — без libc/getuid. Рендерим `--user`.
        let mut config = self.config.clone();
        if config.run_as.is_none() {
            use std::os::unix::fs::MetadataExt;
            if let Ok(meta) = std::fs::metadata(&dir) {
                config.run_as = Some(format!("{}:{}", meta.uid(), meta.gid()));
            }
        }

        // Ожидаемый peer-uid для SO_PEERCRED-гейта accept'а (спека §4.3 инвариант 6). ЕДИНЫЙ источник
        // истины с рендером плана: контейнер бежит под `--user <uid>:<gid>` ровно из `config.run_as`
        // (mod.rs рендерит тот же `config.run_as`), а при rootless-Podman + `--userns=keep-id` его процесс
        // виден ХОСТ-ядру (через `SO_PEERCRED` на host-сокете) под ТЕМ ЖЕ host-uid. Выше `run()` уже
        // дефолтит `run_as` в host-uid дир-владельца, если был `None`, — так что здесь в норме Some(numeric).
        // НАМЕРЕННО без фолбэка на дир-владельца при непарсящемся uid: иначе мисконфиг `run_as`
        // ("alice:alice" / нечисловой) тихо гейтил бы против ДРУГОГО uid, чем реально рендерится в `--user`.
        // `None` (run_as отсутствует ⇒ `--user` не рендерится ⇒ image-USER без host-uid всё равно не откроет
        // 0600-сокеты; ИЛИ нечисловой uid) → peer-гейт fail-closed дропнет ЛЮБОЕ соединение (безопасно).
        // ⚠ Если будущий срез задаёт `run_as` НЕ-host-uid без keep-id, синхронизировать с host-видимым uid.
        let expected_uid: Option<u32> = config
            .run_as
            .as_deref()
            .and_then(|s| s.split(':').next())
            .and_then(|u| u.parse::<u32>().ok());
        // Каталог сокетов — owner-only (0700): defense-in-depth поверх 0600-сокетов (чужой не зайдёт даже
        // в каталог). Best-effort (FS без unix-прав не валит прогон).
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)) {
                tracing::warn!(dir = %dir.display(), error = %e, "sandbox: не удалось сузить права каталога сокетов до 0700");
            }
        }

        let egress_path = self.socket_path(SOCKET_EGRESS);
        let act_path = self.socket_path(SOCKET_ACT);
        let event_path = self.socket_path(SOCKET_EVENT);

        // Bind с хардненингом. Частичный сбой (2-й/3-й сокет упал) → снять уже забинженные (не оставляем
        // болтающиеся сокеты).
        let egress_l = Self::bind_hardened(&egress_path)?;
        let act_l = match Self::bind_hardened(&act_path) {
            Ok(l) => l,
            Err(e) => {
                let _ = std::fs::remove_file(&egress_path);
                return Err(e);
            }
        };
        let event_l = match Self::bind_hardened(&event_path) {
            Ok(l) => l,
            Err(e) => {
                let _ = std::fs::remove_file(&egress_path);
                let _ = std::fs::remove_file(&act_path);
                return Err(e);
            }
        };

        // Serve-таски: каждый обслуживает ОДНО легитимное соединение контейнера реальным backend'ом.
        // accept-LOOP, а не одиночный accept: отвергнутый по peer-uid импостор (защита-в-глубину на случай
        // ослабленных 0600/0700) НЕ должен лишить легитимный контейнер сокета — продолжаем слушать. Выход:
        // обслужили валидного пира (соединение закрылось) ЛИБО accept упал. Контейнер открывает РОВНО одно
        // соединение на сокет → после serve выходим (break) — не виснем на повторном accept (teardown ждёт
        // join с бюджетом; повторный accept не нужен).
        let egress_proxy = Arc::new(egress_proxy);
        let act_server = Arc::new(act_server);
        let event_srv = Arc::new(EventForwardServer::new(event_out));

        let eg = {
            let p = egress_proxy.clone();
            tokio::spawn(async move {
                loop {
                    let Ok((s, _)) = egress_l.accept().await else {
                        break;
                    };
                    if peer_authorized(&s, expected_uid) {
                        serve_egress(AfUnixTransport::new(s), &p).await;
                        break;
                    }
                    tracing::warn!(socket = SOCKET_EGRESS, "sandbox: соединение отвергнуто — peer-uid != run_as-uid (SO_PEERCRED, спека §4.3.6)");
                }
            })
        };
        let ac = {
            let s = act_server.clone();
            tokio::spawn(async move {
                loop {
                    let Ok((st, _)) = act_l.accept().await else {
                        break;
                    };
                    if peer_authorized(&st, expected_uid) {
                        serve_act(AfUnixTransport::new(st), &s).await;
                        break;
                    }
                    tracing::warn!(socket = SOCKET_ACT, "sandbox: соединение отвергнуто — peer-uid != run_as-uid (SO_PEERCRED, спека §4.3.6)");
                }
            })
        };
        let ev = {
            let s = event_srv.clone();
            tokio::spawn(async move {
                loop {
                    let Ok((st, _)) = event_l.accept().await else {
                        break;
                    };
                    if peer_authorized(&st, expected_uid) {
                        s.serve(AfUnixTransport::new(st)).await;
                        break;
                    }
                    tracing::warn!(socket = SOCKET_EVENT, "sandbox: соединение отвергнуто — peer-uid != run_as-uid (SO_PEERCRED, спека §4.3.6)");
                }
            })
        };

        // Spawn `podman run … --sandbox-child …` и ждать выхода контейнера (агент-loop завершился).
        let plan = sandbox_run_plan_with_cmd(&config, &child.to_argv());
        // sandbox-exec-lint: allow podman-launch — ЗАПУСК САМОЙ ПЕСОЧНИЦЫ (podman), НЕ exec-команды агента
        // (§5.2 инверсия: команды агента бегут ВНУТРИ контейнера, sandbox/exec_child.rs; host их не спавнит).
        let status_res = tokio::process::Command::new(&plan.argv[0])
            .args(&plan.argv[1..])
            .status()
            .await
            .map_err(|e| format!("spawn podman ({}): {e}", plan.argv[0]));

        // Контейнер вышел → его соединения закрыты → serve-таски сами завершатся (recv→None — повиснуть
        // не на чем). Даём им ДОТЕЧЬ (особенно event-релей: контейнер сделал `drain.await` ДО выхода →
        // все события уже в event.sock; serve должен их дочитать и релейнуть на десктоп, иначе теряется
        // хвост). Bounded await — если за бюджет не дотекли (залипший десктоп-peer), детачим (teardown
        // снесёт сокеты, оборвав релей) и не виснем.
        let join_all = async {
            let _ = eg.await;
            let _ = ac.await;
            let _ = ev.await;
        };
        if tokio::time::timeout(std::time::Duration::from_secs(3), join_all)
            .await
            .is_err()
        {
            tracing::warn!("sandbox: serve-таски не дотекли за 3с после выхода контейнера — детач");
        }
        // Teardown: снять сокеты (каталог per-run оставляем — его жизненный цикл у вызывающего).
        for p in [&egress_path, &act_path, &event_path] {
            let _ = std::fs::remove_file(p);
        }

        status_res
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actuator::{Action, DispatchOutcome};
    use crate::agent::connect::channel_pair;
    use crate::agent::ToolError;
    use crate::net::{EgressFeature, NetError, RunCtx};
    use crate::sandbox::act::WireAction;
    use crate::sandbox::proxy::{BackendResponse, EgressBudget, Verb};
    use async_trait::async_trait;
    use serde_json::Value;
    use std::sync::Mutex;

    /// Bind сужает права сокета до 0600 (спека §4.2/§4.3 — per-run сокеты owner-only). Регресс на случай,
    /// если кто-то вернёт сырой `UnixListener::bind` мимо `bind_hardened`.
    #[tokio::test]
    async fn bind_hardened_sets_socket_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("egress.sock");
        let _l = SandboxRunner::bind_hardened(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "сокет owner-only (0600)");
    }

    /// Bind ОТКАЗЫВАЕТСЯ удалять НЕ-сокет по пути (не трём чужой файл) — переиспользует `prepare_socket_path`.
    #[tokio::test]
    async fn bind_hardened_refuses_non_socket() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("act.sock");
        std::fs::write(&path, b"chuzhoy fail").unwrap();
        assert!(
            SandboxRunner::bind_hardened(&path).is_err(),
            "не-сокет по пути → отказ"
        );
        assert!(path.exists(), "чужой файл НЕ удалён");
    }

    /// Fail-closed-матрица сопоставления peer-uid (чистая логика, любая ОС — без сокета/`SO_PEERCRED`).
    /// Авторизуем ТОЛЬКО при достоверном равенстве; любое `None` (неизвестный ожидаемый ИЛИ нечитаемый
    /// peer-cred) → отказ. Регресс на случай, если кто-то ослабит сравнение до «равны ИЛИ неизвестны».
    #[test]
    fn uid_matches_is_fail_closed() {
        assert!(
            uid_matches(Some(1000), Some(1000)),
            "равные uid → авторизован"
        );
        assert!(!uid_matches(Some(1000), Some(1001)), "разные uid → отказ");
        assert!(
            !uid_matches(None, Some(1000)),
            "неизвестный ожидаемый → отказ"
        );
        assert!(
            !uid_matches(Some(1000), None),
            "нечитаемый peer-cred → отказ"
        );
        assert!(!uid_matches(None, None), "оба неизвестны → отказ");
    }

    /// **Tier-1 (Linux):** на РЕАЛЬНОЙ паре `UnixListener` ↔ `UnixStream` `SO_PEERCRED` читает наш uid;
    /// соединение того же uid АВТОРИЗУЕТСЯ, заведомо-чужой ожидаемый uid — ОТВЕРГАЕТСЯ (mismatch-ветка БЕЗ
    /// привилегий — через неверный `expected`), неизвестный ожидаемый — тоже (fail-closed). Кросс-uid
    /// РЕАЛЬНЫМ процессом другого пользователя — **Tier-2** (нужны привилегии/второй uid, здесь недостижимо;
    /// см. §8.2 podman-gated). На не-Linux `peer_uid`=`None` → всё отвергается (sandbox Linux-only, §9), потому
    /// тест Linux-gated.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn peer_authorized_accepts_same_uid_rejects_mismatch() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("peer.sock");
        let listener = SandboxRunner::bind_hardened(&path).unwrap();
        // accept (сервер) и connect (клиент) — оба наш процесс ⇒ один uid (как контейнер под keep-id).
        let (accepted, _client) =
            tokio::join!(async { listener.accept().await.unwrap().0 }, async {
                UnixStream::connect(&path).await.unwrap()
            },);
        let me = unsafe { libc::getuid() };
        assert_eq!(peer_uid(&accepted), Some(me), "SO_PEERCRED читает наш uid");
        assert!(
            peer_authorized(&accepted, Some(me)),
            "тот же uid → авторизован"
        );
        assert!(
            !peer_authorized(&accepted, Some(me.wrapping_add(1))),
            "чужой ожидаемый uid → отвергнут"
        );
        assert!(
            !peer_authorized(&accepted, None),
            "неизвестный ожидаемый → отвергнут (fail-closed)"
        );
    }

    #[test]
    fn child_argv_is_positional_and_safe() {
        let a = SandboxChildArgs {
            run_id: "run7".into(),
            base_url: "http://llm:8080".into(),
            model: "qwen".into(),
            context_window: 8192,
            task: "сделай это; rm -rf /".into(), // спецсимволы — но argv (не шелл), безопасно
        };
        assert_eq!(
            a.to_argv(),
            vec![
                "--sandbox-child",
                "run7",
                "http://llm:8080",
                "qwen",
                "8192",
                "сделай это; rm -rf /"
            ]
        );
    }

    /// Egress serve-хелпер: request → GuardedProxy(mock) → response (Tier-1, ChannelTransport).
    #[tokio::test]
    async fn serve_egress_handles_one_request() {
        struct Ok200;
        #[async_trait]
        impl EgressBackend for Ok200 {
            async fn fetch(
                &self,
                _v: Verb,
                _u: &str,
                _f: EgressFeature,
                _b: Option<&Value>,
                _c: RunCtx,
            ) -> Result<BackendResponse, NetError> {
                Ok(BackendResponse {
                    status: 200,
                    content_type: Some("application/json".into()),
                    body: b"{\"ok\":true}".to_vec(),
                })
            }
        }
        let (client, host) = channel_pair();
        let proxy = GuardedProxy::new(
            Ok200,
            1,
            vec![EgressFeature::Chat],
            EgressBudget::new(1 << 20, 4),
        );
        let srv = tokio::spawn(async move { serve_egress(host, &proxy).await });

        // Клиент шлёт egress/post через шим.
        let shim = crate::sandbox::proxy::ProxyGuardedClient::new(client);
        let resp = shim
            .post_json(
                "http://llm:8080/v1/chat/completions",
                EgressFeature::Chat,
                &serde_json::json!({"x":1}),
            )
            .await
            .unwrap();
        assert_eq!(resp.status, 200);
        assert!(resp.body.contains("ok"));
        drop(shim); // закрыть соединение → serve выйдет.
        srv.await.unwrap();
    }

    /// Act serve-хелпер: WireAction → HostActServer(mock) → запись зафиксирована (Tier-1).
    #[tokio::test]
    async fn serve_act_handles_one_request() {
        struct Capture(Mutex<Option<Action>>);
        #[async_trait]
        impl ActuatorBackend for Arc<Capture> {
            async fn act(&self, action: &Action) -> Result<DispatchOutcome, ToolError> {
                *self.0.lock().unwrap() = Some(action.clone());
                Ok(DispatchOutcome::Applied("ок".into()))
            }
        }
        let cap = Arc::new(Capture(Mutex::new(None)));
        let server = HostActServer::new(cap.clone());
        let (client, host) = channel_pair();
        let srv = tokio::spawn(async move { serve_act(host, &server).await });

        let shim = crate::sandbox::act::ProxyActuator::new(client);
        let out = shim
            .dispatch(&Action::note_create("Notes/A.md", "тело"))
            .await
            .unwrap();
        assert_eq!(out, DispatchOutcome::Applied("ок".into()));
        assert_eq!(
            cap.0.lock().unwrap().as_ref().unwrap().target.rel(),
            "Notes/A.md"
        );
        // wire round-trip санити (WireAction в импортах — пинит контракт).
        let _ = WireAction::try_from(&Action::note_edit("X.md", "y")).unwrap();
        drop(shim);
        srv.await.unwrap();
    }
}
