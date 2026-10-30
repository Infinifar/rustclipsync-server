# RustClipSync Server

Small Rust server for the **text** subset used by [Omaclipsync](https://github.com/infinifar/omaclipsync). It keeps one current clipboard profile and content-addressed text history in SQLite. It is not a general SyncClipboard, WebDAV, or file server.

## 中文：运行

要求 Docker Compose。创建只存在于本机的账户配置：

```sh
umask 077
printf 'RCS_USER=%s\n' 'your-user' > .env
printf 'RCS_DB_KEY=%s\n' "$(openssl rand -hex 32)" >> .env
printf '%s' 'replace-with-a-long-random-password' > password.secret
sudo chgrp 10001 password.secret
chmod 0440 password.secret
docker compose up --build -d
docker compose ps
```

请换成自己的用户名和至少 12 字符的随机密码。`.env` 的 `RCS_DB_KEY` 是 32 字节随机值的 64 位十六进制表示，必须安全备份；丢失密钥后无法恢复数据库。`.env` 必须限制为可信用户访问，且不得提交。密钥作为容器环境变量，可被 Docker 管理员、Docker API 用户及有权读取进程环境的用户看到。容器以 GID 10001 读取密码文件；父目录应限制访问。已有数据库不能直接修改密钥值，需先进行数据库重新加密。Compose 只把 `8090` 映射到宿主机 `127.0.0.1`；远程 Omaclipsync 应通过现有 TLS 反向代理访问，并将 `/SyncClipboardHub` 的 WebSocket 升级转发到本服务。

配置 Omaclipsync 时填写 HTTPS 基础 URL、用户名和密码。服务支持 URL 路径前缀，但代理必须统一转发 `/`、`/SyncClipboard.json`、`/file/`、`/api/history/` 和 `/SyncClipboardHub`。`GET /healthz` 不要求认证，仅返回 `ok`；其余接口需要 Basic 认证。

源码运行：

```sh
RCS_USER=your-user RCS_PASSWORD=replace-with-a-long-random-password RCS_DB_KEY=your-64-hex-digit-key \
  RCS_DATA_DIR=./data cargo run --release
```

默认监听 `127.0.0.1:8090`，容器内设置为 `0.0.0.0:8090`。`RCS_PASSWORD_FILE` 可替代 `RCS_PASSWORD`，适合 Docker secrets。数据库及 WAL 使用 SQLCipher 加密；密钥通过 `RCS_DB_KEY` 提供。首次迁移前的明文备份及历史备份仍是明文，不会自动删除。

升级已有明文数据库时，先备份数据卷和密钥，停止所有访问数据库的进程，然后用新镜像执行一次 `docker compose run --rm --no-deps rustclipsync --migrate-db`，成功后再 `docker compose up -d`。迁移会校验源数据库，生成并校验加密副本，再将原文件保留为 `clipsync.db.plaintext-backup`。迁移中断时保留的临时文件或备份需要先人工检查，不要覆盖。SQLCipher 加密保护静态存储，进程运行期间及持有密钥的用户仍可读取数据；它不是端到端加密。

历史默认保留 7 天，可在 Compose 的 `.env` 中设置 `RCS_HISTORY_RETENTION_DAYS=30`（允许 1 至 3650 天）。服务启动时和之后每小时清理过期历史，始终保留当前剪贴板及时间无法解析的记录。过期时间按最后一次同步的时间计算；修改 `.env` 后需要重新创建容器。清理会永久删除对应历史，SQLite 文件或备份中可能仍有残留，因此不应将此功能视为安全擦除。

## 协议和限制

实现 `GET /`、`GET`/`PUT /SyncClipboard.json`、`PUT`/`GET`/`HEAD /file/{name}`、`GET /api/history/Text-{hash}` 与 `/data`、SignalR v1 的协商和 JSON WebSocket 事件。只接受 UTF-8 `Text`。文本上限 10 MiB，元数据上限 256 KiB；长文本文件名必须是安全的 `.txt` 名称。服务端计算 SHA-256 哈希，按 UTF-16 代码单元填写 `size`，由服务端填写 `syncedAt`。长文本先暂存再提交；提交成功后才广播。未提交上传最多 16 个，并在后续上传或服务启动时清理超过一小时的对象。没有多账户、图片、普通文件、文件组、完整历史管理 API、通用 WebDAV 或 S3 功能。

SQLite 使用 WAL 和 FULL 同步。备份前停止容器，让数据库及 WAL 处于稳定状态：

```sh
docker compose stop
docker run --rm -v rustclipsync_data:/data:ro -v "$PWD":/backup busybox \
  tar -C /data -czf /backup/rustclipsync-backup.tar.gz .
docker compose start
```

升级前保存备份。回退时先 `docker compose stop`，恢复该卷的备份，使用原镜像/源码版本重新启动。不要直接将旧版 SyncClipboard 的数据目录挂载到本服务；它不会自动迁移数据。卸载时先 `docker compose down`；数据卷会保留，只有确定不再需要数据时才手动删除 `rustclipsync_data` 卷。

## English

Create `.env` with `RCS_USER` and `RCS_DB_KEY` (64 hexadecimal characters from 32 random bytes), and a private `password.secret` readable by GID 10001. Protect and back up `.env`: the database is unreadable without its key. Container environment variables are visible to Docker administrators and other users with sufficient access. Changing the key value alone does not re-encrypt existing data. Set `RCS_HISTORY_RETENTION_DAYS` to configure retention (default 7 days). The published port is bound to loopback. For an existing plaintext database, stop the service, back it up, then run `docker compose run --rm --no-deps rustclipsync --migrate-db` before restarting. The original plaintext database remains as `clipsync.db.plaintext-backup`; protect and handle it separately. SQLCipher encrypts storage, not end-to-end traffic. Put a TLS reverse proxy in front of the service and forward WebSocket upgrades.

## Development

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

The reference client is the local Omaclipsync Rust core. Android and other third-party clients have not been tested. License: MIT.
