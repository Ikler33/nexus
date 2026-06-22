# syntax=docker/dockerfile:1
#
# Multi-stage сборка `nexus-agentd` — headless агент-сервиса (DEPLOY-3). Десктоп/Tauri НЕ собирается:
# `cargo build -p nexus-agentd` тянет только agentd + ядро `nexus-core` (без `nexus-desktop` и его
# webkit/gtk-зависимостей). Образ — также база Фазы-2 Podman-песочницы (`--network=none` + GuardedProxy).
#
# Сборка:  docker build -t nexus-agentd:local .
# Запуск:  docker run -d --name nexus-agentd -v /host/vault:/vault \
#            -e NEXUS_AGENTD_CONNECT_SOCKET=/vault/.nexus/agentd.sock nexus-agentd:local
# (AF_UNIX-коннектор по bind-mount работает на Linux-хосте; на macOS Docker Desktop сокет через
#  virtiofs не пробрасывается — там запускайте agentd нативно или ждите WS-транспорт.)

FROM rust:1-bookworm AS builder
WORKDIR /src
COPY . .
# Только agentd + его зависимости (nexus-core); nexus-desktop/Tauri не компилируется.
RUN cargo build --release -p nexus-agentd

FROM debian:bookworm-slim AS runtime
# ca-certificates — для исходящего TLS агента (chat/embed/web через GuardedClient).
# git — для exec-таргета GitOp агента ВНУТРИ песочницы (SANDBOX-6c) + pre-op-ref undo (6c-3); ~+15МБ.
#   Без него `git.op`/реальный `git reset --hard` в контейнере структурно невозможны.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates git \
 && rm -rf /var/lib/apt/lists/*
# Непривилегированный системный пользователь (контейнер не бежит под root).
RUN useradd --system --uid 10001 --create-home --home-dir /home/nexus nexus
COPY --from=builder /src/target/release/nexus-agentd /usr/local/bin/nexus-agentd
USER nexus
# vault монтируется как том; коннектор (AF_UNIX) включается на запуске через
# NEXUS_AGENTD_CONNECT_SOCKET (default-OFF — не зашит в образ).
ENV NEXUS_VAULT=/vault \
    RUST_LOG=info
VOLUME ["/vault"]
ENTRYPOINT ["/usr/local/bin/nexus-agentd"]
