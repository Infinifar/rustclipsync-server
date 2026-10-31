# RustClipSync Server

RustClipSync 是面向个人设备的轻量 Rust 文本剪贴板同步服务，目标是较低的内存占用。它采用自有 RustClipSync V1 协议：离线期间只追上最新内容，通过消息 UUID、服务端版本和设备游标处理重试与重复通知。

## 客户端与协议

- Android：[Fcitx5 Android 输入法 fork](https://github.com/Infinifar/fcitx5-android-bibi-keyboard)，内置 RustClipSync 同步模块，保留说点啥语音输入支持。
- Omarchy：[Omaclipsync 插件](https://github.com/Infinifar/omaclipsync)，使用支持 V1 的版本。
- 其他客户端：[V1 接口协议与适配说明](PROTOCOL-V1.md)。初次登记从当前版本开始，不回放旧剪贴板；断线期间只追上最新状态，本地待发送内容与远端冲突时需用户确认。剪贴板写入与本地游标不能原子提交，异常崩溃时不保证严格恰好一次。

只同步 UTF-8 文本，单条上限 10 MiB，不支持图片、文件或多账户。敏感文本默认不传输；只有来源应用明确标记的敏感内容才能被识别。

## 与 SyncClipboard 的关系

本项目早期实现过 [SyncClipboard](https://github.com/Jeric-X/SyncClipboard) 文本协议兼容；当前 V1 是独立的 Rust 协议实现，**不兼容 SyncClipboard 客户端或服务端**，也不是其服务端源码的 Rust 移植。

| 方面 | RustClipSync V1 | SyncClipboard |
| --- | --- | --- |
| 范围 | 轻量文本同步；需要专门适配的客户端 | 跨平台客户端、文本、图片、历史及其他存储后端，功能更广 |
| 离线与重复 | 每次复制使用 UUID；服务端版本和每设备游标处理重试、回声和最新值；离线不补发中间历史 | 有自己的同步与历史机制；具体去重行为取决于版本与客户端 |
| 隐私 | HTTPS 传输、SQLCipher 数据库加密、设备独立令牌可撤销；敏感标记默认不传输 | 支持自行部署及多种后端；安全和数据保护取决于部署及客户端配置 |
| 取舍 | 功能和兼容客户端较少；服务端可读取正文，尚无端到端加密 | 平台和功能覆盖更广；资源使用及安全能力应以实际版本和部署为准 |

没有对两者进行同条件性能或安全审计，不据此宣称绝对优势。一次本机回环测试中，服务端空闲 RSS 约 9.4 MiB；5 分钟合成负载中 RSS 约从 9.45 增至 11.74 MiB、无请求错误。这些数值不代表公网、所有负载或长期运行结果。

## Docker Compose 部署

需要 Docker Compose、可用的 HTTPS 反向代理或 Cloudflare Tunnel。下面的 `docker-compose.yml` 固定到已发布的 V1 镜像；`latest` 可能仍是旧协议版本。容器仅在宿主机 `127.0.0.1:8090` 开放端口：

```yaml
services:
  rustclipsync:
    image: ghcr.io/infinifar/rustclipsync-server:sha-c57194e
    restart: unless-stopped
    ports:
      - "127.0.0.1:8090:8090"
    environment:
      RCS_USER: ${RCS_USER:?set RCS_USER}
      RCS_PASSWORD_FILE: /run/secrets/rcs_password
      RCS_DB_KEY: ${RCS_DB_KEY:?set RCS_DB_KEY to 64 hexadecimal characters}
      RCS_HISTORY_RETENTION_DAYS: ${RCS_HISTORY_RETENTION_DAYS:-7}
    secrets:
      - rcs_password
    volumes:
      - clipsync_data:/data
    security_opt:
      - no-new-privileges:true
    cap_drop:
      - ALL
secrets:
  rcs_password:
    file: ./password.secret
volumes:
  clipsync_data:
    name: rustclipsync_data
```

在文件所在目录创建配置，替换账户名并交互输入至少 12 字节的随机密码：

```sh
umask 077
printf 'RCS_USER=%s\n' 'your-user' > .env
printf 'RCS_DB_KEY=%s\n' "$(openssl rand -hex 32)" >> .env
read -rs -p 'Password: ' rcs_password; printf '\n'
printf '%s' "$rcs_password" > password.secret
unset rcs_password
sudo chgrp 10001 password.secret
chmod 0440 password.secret
docker compose up -d
docker compose ps
curl http://127.0.0.1:8090/healthz
```

`.env` 中的 `RCS_DB_KEY` 是 32 字节密钥的 64 位十六进制表示，必须妥善备份；丢失后无法恢复数据库。Docker 管理员可读取容器环境变量。单独更改密钥不会重新加密数据库。账号密码用于登记和管理设备，同步使用独立设备令牌。不要将 `.env`、密码文件或备份提交到仓库。

将 HTTPS 代理指向 `http://127.0.0.1:8090`，转发 `/healthz` 和 `/v1/`；长轮询最长 25 秒，代理超时应更长。客户端填写 HTTPS 根地址、账户和密码进行配对。HTTPS 保护传输，但服务端运行时可读取正文；当前不提供端到端加密。

历史默认保留 7 天；可在 `.env` 中设置 `RCS_HISTORY_RETENTION_DAYS` 为 1–3650 天，并重新创建容器。当前文本始终保留，消息 ID 和取消回执至少保留 30 天；清理不等于安全擦除。

## 升级、备份与恢复

从旧协议升级前，停止服务并备份完整数据卷、密钥、密码和旧镜像；旧协议数据不会自动作为 V1 消息发送。旧版客户端需要更换为 V1 客户端。

明文数据库迁移前先停止服务并备份，然后运行 `docker compose run --rm --no-deps rustclipsync --migrate-db`。迁移保留明文备份，需单独保护或在确认后处理。

恢复旧 V1 数据库备份后，在服务停止且无其他进程使用数据库时运行 `docker compose run --rm --no-deps rustclipsync --rotate-v1-epoch`。这会撤销设备令牌并更换实例标识，所有设备须重新配对。回退旧服务时应恢复与旧镜像匹配的数据库备份和客户端。

## 开发验证

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
python3 benchmarks/v1_benchmark.py target/release/rustclipsync-server
```

基准使用临时加密数据库与合成文本，不代表公网性能。服务端采用 MIT 许可；客户端保留各自许可。
