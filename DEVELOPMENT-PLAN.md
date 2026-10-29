# RustClipSync Server 开发计划

## 目标

用 Rust 实现轻量、低内存占用的单账户文本剪贴板服务端，以 Docker 部署。当前兼容目标是本机 Omaclipsync 的 HTTP 和 SignalR 使用方式。Android 客户端、图片、文件、文件组、完整历史管理 API、通用 WebDAV 和 S3 不在本次范围内。真实服务切换由用户决定。

## 实施顺序

1. 核对本机 Omaclipsync 请求：`GET /`、`GET`/`PUT /SyncClipboard.json`、长文本 `PUT /file/{name}`、历史对象查询/下载，以及 SignalR 协商、WebSocket 握手和 `RemoteProfileChanged`。
2. 实现受限 Basic 认证、字段和哈希验证、10 MiB 文本与 256 KiB 元数据上限。短文本直接入库；长文本先暂存，元数据校验成功后通过 SQLite 事务切换当前内容并广播。失败提交不改变当前内容。
3. 实现最小 SignalR JSON 协议：协商令牌、WebSocket 认证、握手、心跳、事件广播；断线后由 Omaclipsync 重新拉取当前状态。
4. 使用 SQLite WAL 保存当前状态与按哈希查找的文本数据。控制未提交上传数量与过期时间；提供持久卷、非 root 容器、健康检查、备份和回退说明。
5. 运行格式、Clippy、单元和接口测试；在本机隔离环境用合成文本检查短文本、长文本、认证、错误输入、重启恢复和 SignalR。测量容器资源。只有用户在真实设备上配置 Omaclipsync 后，才能确认桌面端到端兼容。
6. 完成 README、Compose 示例、CI 和许可证，创建 GitHub 仓库并推送源码。

## 验收与限制

- 服务端只接受 `Text`，拒绝其他类型；哈希为 UTF-8 正文 SHA-256 大写十六进制，`size` 为 UTF-16 代码单元数。
- Basic 认证仅配合本机回环地址或 TLS 反向代理使用；容器端口默认只绑定宿主机回环地址。
- 长文本历史查询只实现 Omaclipsync 所需的单对象查询和下载，不承诺完整上游历史协议。
- GitHub 推送需有可用的仓库创建与写入凭据；推送结果以远程提交为准。
