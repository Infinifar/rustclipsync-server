# RustClipSync Server

轻量的 Rust 剪贴板同步服务，使用专用的 RustClipSync V1 协议。服务保存当前文本和有限期限的加密历史，适用于对应版本的 Omaclipsync 和 Android 输入法同步模块。**V1 不兼容 SyncClipboard 旧协议**；升级服务端前，必须先准备好 V1 客户端及回滚备份。

协议、认证、断线恢复和冲突处理详见 [PROTOCOL-V1.md](PROTOCOL-V1.md)。

## 部署

需要 Docker Compose、一个指向本机服务的 HTTPS 反向代理，以及已适配 V1 的客户端。Compose 将端口绑定为 `127.0.0.1:8090:8090`；例如 Cloudflare Tunnel 的后端可以使用 `http://127.0.0.1:8090`。代理只需转发 `/healthz` 和 `/v1/`，长轮询单次最长 25 秒，应设置大于该值的代理超时。

首次部署时在 Compose 文件所在目录创建私有配置：

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

请使用自己的账户名和至少 12 字节的随机密码。`RCS_DB_KEY` 是 32 字节随机密钥的 64 位十六进制表示，直接配置在 `.env` 中；必须妥善备份，丢失后无法恢复数据库。不要将 `.env`、密码文件或备份提交至 Git。容器内进程以 GID 10001 读取密码文件；Docker 管理员及有权限读取容器环境的用户可以取得环境变量。单独修改密钥不会重新加密已有数据库。

默认监听 `127.0.0.1:8090`；容器内监听 `0.0.0.0:8090`，宿主机映射仍限定回环地址。`GET /healthz` 无需认证；设备登记和管理使用账户密码，设备同步使用独立随机令牌。所有远程访问必须使用 HTTPS。未实施端到端加密，服务端运行时能看到同步文本。

历史文本默认保留 7 天，在 `.env` 中设置 `RCS_HISTORY_RETENTION_DAYS=30` 可改为 1–3650 天；修改后重新创建容器生效。启动后和每小时清理过期历史，始终保留当前文本。已处理消息 ID 与取消回执至少保留 30 天；客户端自动重试期限为 7 天。历史清理不是安全擦除，旧备份和数据库页仍可能包含加密残留。

V1 用消息 UUID 和设备游标处理重试及重复，不使用旧的 `RCS_DEDUP_WINDOW_SECONDS`。相同文本的两次真实复制是不同事件。敏感内容是否发送由客户端控制，默认关闭；客户端只能识别来源应用明确标记的敏感内容。

## 升级与恢复

从旧服务升级前，停止使用数据库的所有进程，备份完整数据卷及 `.env` 中的密钥，并确认客户端支持 V1。旧协议的数据库记录不会作为 V1 消息自动发送；新设备第一次配对从当前 V1 版本开始，不覆盖已有本地剪贴板。旧记录仍留在加密数据库中，按原有时间规则清理，升级不会主动删除旧的当前记录。

如果原数据库是明文，先备份并停止所有服务，再执行：

```sh
docker compose run --rm --no-deps rustclipsync --migrate-db
```

迁移成功后原文件仍作为 `clipsync.db.plaintext-backup` 保留，需单独保护或在确认备份后处理。此迁移只处理存储加密，不转换旧同步协议。

还原旧 V1 数据库备份后，**保持服务停止**，对恢复的数据库执行一次：

```sh
docker compose run --rm --no-deps rustclipsync --rotate-v1-epoch
```

该命令更换数据库实例标识并撤销全部 V1 设备令牌，不删除当前文本或历史；随后所有设备需要重新配对。不要在服务正在使用数据库时执行。回退旧版本需同时恢复对应的数据库备份和镜像，并使用与其兼容的客户端。

## 本地开发与验证

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
python3 benchmarks/v1_benchmark.py target/release/rustclipsync-server
```

基准使用临时加密数据库和合成文本，只测试本机 HTTP 回环，不代表 HTTPS、Cloudflare 或公网速度。服务端为 MIT 许可；客户端各自保留原项目许可。
