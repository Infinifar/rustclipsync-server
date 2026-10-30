# RustClipSync Server

Small Rust server for the **text** subset used by [Omaclipsync](https://github.com/infinifar/omaclipsync). It keeps one current clipboard profile and content-addressed text history in SQLite. It is not a general SyncClipboard, WebDAV, or file server.

## 中文：运行

要求 Docker Compose。创建只存在于本机的账户配置：

```sh
printf 'RCS_USER=%s\n' 'your-user' > .env
umask 077
printf '%s' 'replace-with-a-long-random-password' > password.secret
docker compose up --build -d
docker compose ps
```

请换成自己的用户名和至少 12 字符的随机密码。`password.secret`、`.env` 和数据库均被 Git 忽略。Compose 只把 `8090` 映射到宿主机 `127.0.0.1`；远程 Omaclipsync 应通过现有 TLS 反向代理访问，并将 `/SyncClipboardHub` 的 WebSocket 升级转发到本服务。不要直接在公网 HTTP 上发送 Basic 凭据。

配置 Omaclipsync 时填写 HTTPS 基础 URL、用户名和密码。服务支持 URL 路径前缀，但代理必须统一转发 `/`、`/SyncClipboard.json`、`/file/`、`/api/history/` 和 `/SyncClipboardHub`。`GET /healthz` 不要求认证，仅返回 `ok`；其余接口需要 Basic 认证。

源码运行：

```sh
RCS_USER=your-user RCS_PASSWORD=replace-with-a-long-random-password \
  RCS_DATA_DIR=./data cargo run --release
```

默认监听 `127.0.0.1:8090`，容器内设置为 `0.0.0.0:8090`。`RCS_PASSWORD_FILE` 可替代 `RCS_PASSWORD`，适合 Docker secrets。数据文件是 `RCS_DATA_DIR/clipsync.db` 及其 WAL 文件；请使用持久卷。

历史默认保留 7 天，可在 Compose 的 `.env` 中设置 `RCS_HISTORY_RETENTION_DAYS=30`（允许 1 至 3650 天）。服务启动时和之后每小时清理过期历史，始终保留当前剪贴板及时间无法解析的记录。过期时间按最后一次同步的时间计算；修改 `.env` 后需要重新创建容器。清理会永久删除对应历史，SQLite 文件或备份中可能仍有残留，因此不应将此功能视为安全擦除。

## 协议和限制

实现 `GET /`、`GET`/`PUT /SyncClipboard.json`、`PUT`/`GET`/`HEAD /file/{name}`、`GET /api/history/Text-{hash}` 与 `/data`、SignalR v1 的协商和 JSON WebSocket 事件。只接受 UTF-8 `Text`。文本上限 10 MiB，元数据上限 256 KiB；长文本文件名必须是安全的 `.txt` 名称。服务端计算 SHA-256 哈希，按 UTF-16 代码单元填写 `size`，由服务端填写 `syncedAt`。长文本先暂存再提交；提交成功后才广播。未提交上传最多 16 个，并在后续上传或服务启动时清理超过一小时的对象。历史对象目前无限保留；请监控磁盘容量。没有多账户、图片、普通文件、文件组、完整历史管理 API、通用 WebDAV 或 S3 功能。

SQLite 使用 WAL 和 FULL 同步。备份前停止容器，让数据库及 WAL 处于稳定状态：

```sh
docker compose stop
docker run --rm -v rustclipsync_data:/data:ro -v "$PWD":/backup busybox \
  tar -C /data -czf /backup/rustclipsync-backup.tar.gz .
docker compose start
```

升级前保存备份。回退时先 `docker compose stop`，恢复该卷的备份，使用原镜像/源码版本重新启动。不要直接将旧版 SyncClipboard 的数据目录挂载到本服务；它不会自动迁移数据。卸载时先 `docker compose down`；数据卷会保留，只有确定不再需要数据时才手动删除 `rustclipsync_data` 卷。

## English

Run `docker compose up --build -d` after creating `.env` with `RCS_USER` and a private `password.secret` containing a random password of at least 12 characters. The published port is bound to loopback. Put a TLS reverse proxy in front of it and forward WebSocket upgrades. The service only supports text and the Omaclipsync protocol subset above. Persist and back up the named `rustclipsync_data` volume. Stop the container before a filesystem backup. Do not mount an existing SyncClipboard server data directory here.

## Development

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

The reference client is the local Omaclipsync Rust core. Android and other third-party clients have not been tested. License: MIT.
