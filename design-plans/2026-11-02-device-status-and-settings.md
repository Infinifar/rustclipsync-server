# 设备同步状态、排障信息与独立设置页

Written against: `b2e6ae0663c8d93e97846b5b9133e7e6de9916ee`，分支 `codex/device-direction-routing`。

状态：用户已批准实施，并要求完成后部署到本机容器。已按本文改造服务端只读设备元数据与管理页面，验证结果见 ADMINISTRATION.md 和 docs/DIRECTION-VALIDATION.md。

## Evidence chain

- Surface：`GET /admin`；`assets/admin.html` 的 workspace、devices-panel、accounts-panel、sync-settings；脚本 `assets/admin.js` 的 setTab/load/render；样式 `assets/admin.css`。
- 当前情况：设备表只有设备、账户、在线活动、最后活动和操作；重连补发表单位于两个面板之外，因此在设备与账户任务下都出现。用户要求增加方向状态，并将补发移入独立设置页面。
- 数据所有者：`src/admin.rs::DeviceRow/devices` 尚未返回方向；`src/direction.rs` 的 `rcs_direction` 已持久化方向、暂停、版本、上报时间、过期、租约、操作和来源。无需改变客户端同步协议即可投影这些已有数据。
- Design language：保留现有 Omaclipsync 图标、系统字体、浅深色 CSS tokens、原生表格/移动端卡片、原生 dialog、英文默认与中文切换。
- Governing owners：admin.html 负责面板，admin.js 负责渲染和会话隔离，admin.css 负责主题与响应式；admin.rs 负责独立管理员认证与只读设备 DTO。
- Explicit exceptions：None documented。
- 不确定性：没有客户端健康遥测、剪贴板写入成功遥测或 RTT 数据，不能从服务端方向和活动推断这些结果。本轮不新增客户端遥测协议。

## Design decision

保留同一管理工作区，使用“设备 / 账户 / 设置”三个平级页签。设备页呈现服务端实际知道的方向与活动状态；设置页承载全局重连补发及必要的服务端诊断。账户管理、配对与撤销语义保持现状。

设备信息用于说明策略和排障，不提供远程覆盖客户端方向或敏感开关的操作。

## Reuse

- 页签沿用 `.tabs`、`setTab`、`role=tab/tabpanel` 和现有键盘切换实现。
- 设备列表沿用 `.table-wrap`、`.status`、`.muted`、`.small`、`.device-id`、`.row-actions`、移动端 `data-label` 呈现。
- 设置沿用 `sync-settings-form`、数字输入校验、`settingsDirty`、错误提示与审计展示，迁移面板而非复制第二个表单。
- 危险操作沿用现有撤销 dialog、独立管理员 cookie、CSRF、Origin 校验和会话 generation 保护。

## Changes

### 1. 顶部三个页签与设置页面

- `assets/admin.html`：增加 `tab-settings` 与 `settings-panel`，将现有 `.sync-settings` 整体移动到 settings-panel。设备与账户页不再出现补发表单。
- `assets/admin.js`：把 tab 集合统一为 devices/accounts/settings，扩展切换及方向键/Home/End 导航，更新面板 hidden 与 aria-selected。
- 切换页签不丢弃尚未保存的补发输入；现有定时刷新不得覆盖 dirty 输入。登出、会话失效和 bfcache 返回必须清理三个面板中的私有数据。
- 新增的中英文键保持同步；默认英语，保留中文按钮。

### 2. 设备方向与服务器已知状态

在 `GET /api/admin/devices` 增加嵌套 `sync` DTO；仅扩展管理员接口，不改变客户端 `/api/state`、WS 消息、鉴权或分发逻辑。

建议字段（名称在实施时统一，不暴露内部 SQL 行）：

```json
{
  "sync": {
    "direction": "BIDIRECTIONAL",
    "paused": false,
    "reportState": "reported",
    "reportedAt": 1793620000,
    "expiresAt": 1796212000,
    "lastReportSource": "ws",
    "dataLeaseActive": true
  }
}
```

- `direction`：PUSH_ONLY / RECEIVE_ONLY / BIDIRECTIONAL / null。
- `reportState`：reported / unknown / revoked / expired。reported 表示服务端已接受方向，不等于客户端已收到 ACK，也不代表客户端剪贴板工作正常。
- `paused`：独立于方向，显示为“暂停 · 双向”等，避免发明第四个方向枚举。
- `reportedAt`：方向记录的 updated；`expiresAt` 是活动 TTL 截止时间，不能当作令牌过期时间。
- `lastReportSource`：最近方向上报来源 WS / HTTP，不显示为“当前连接通道”。
- `dataLeaseActive`：服务端当前数据租约是否有效；与 60 秒活动在线标记分别解释。租约失效不等于设备令牌已撤销。
- 只读查询以同一个服务端时间投影有效状态；无需为了显示过期状态而更新数据库、刷新 TTL 或唤醒设备。
- 通过 LEFT JOIN 一次取得方向数据，兼容从未上报的设备；避免每行新增查询。

设备表建议列为：设备、账户、同步状态、最近活动、操作。同步状态单元格分行展示“仅发送 / 仅接收 / 双向 / 暂停”和“已上报 / 未上报 / 方向已撤销 / 已过期”；展开只读详情时展示最后上报、最后上报方式、方向版本及数据租约。移动端沿用现有卡片样式，不叠加新框架。

“在线”继续只表示近 60 秒有认证活动。提示语明确：服务端不知道客户端是否真的完成系统剪贴板写入。本地标记敏感内容默认仅进入收件区时，不能被管理页误判为同步故障。

### 3. 设置页：重连补发与排障

- 补发窗口：默认初始值 5 分钟，合法范围 0–1440；0 关闭并清理离线补发。保留仅管理员修改、立即生效、非法值拒绝与变更审计。
- 帮助文字分别说明：非零调整作用于之后写入的记录，已有记录保留写入时 TTL；0 还会禁用已提升的离线补发，但不取消当前有效会话的在线实时消息。
- 配置异常固定兜底 0，明确显示“配置异常：补发已关闭”，不伪装成普通的用户关闭状态。
- 从已有 `sync-settings.metrics` 展示上报成功/失败、WS/HTTP 来源、过滤、未知状态、撤销、补发、配置异常计数。注明计数随服务端进程重启归零，不称为历史统计。
- 保留“最后确认修订号”作为可选设备详情，但注明确认也可能是 skipped，不等于系统剪贴板已写入；不创造并未记录的 ACK 时间。
- 排障文案给出客户端版本匹配、方向/暂停、上报状态、租约、补发配置的检查顺序。没有实际 RTT 和客户端遥测时，不显示伪造延迟、客户端版本或“客户端健康”。

### 4. GitHub 项目入口

- 页头使用 GitHub 图标及可见文字链接，登录前后均可访问；固定指向 `https://github.com/Infinifar/rustclipsync-server`。
- 原生链接在新标签页打开，使用 `rel="noopener noreferrer"`，提供新标签页提示；图标内嵌，无外部素材或跟踪请求。
- 保留语言与退出入口；窄屏换行，点击目标至少 44px，不挤压品牌与操作。

## Security and compatibility

- 所有新增字段与诊断计数继续只允许唯一管理员会话访问；设备 Basic/Bearer 凭据不能读取管理数据。
- 不返回会话 ID、设备令牌、密码、数据库密钥、剪贴板正文或敏感收件区正文；不将会话资料放入 URL/localStorage。
- 方向列为只读：服务端管理员不在这版强制切换客户端方向，也不远程开启敏感同步/自动写入。
- HTML 动态文本继续使用 textContent，验证恶意标签、超长设备标签和账户名不会成为 HTML。
- 保留 cookie Secure/HttpOnly/SameSite、Origin/CSRF、无缓存与 CSP；不为读诊断信息放宽现有权限。
- 方向记录缺失、过期、撤销时按服务端实际有效状态显示；管理页轮询不能刷新设备活动或方向 TTL。
- 不修改数据库中同步方向，不增加后台广播，不改变新旧客户端的协议门禁和现有客户端收发行为。

## Scope

- Inherit：设备列表与详情、设置页、英文/中文文案、桌面及手机布局。
- Verify：账户筛选、设备标签、设备/账户撤销、登录/登出、会话切换与 dirty 表单保护。
- Exclude：客户端遥测、远程方向强制覆盖、敏感开关下发、剪贴板正文展示、E2EE、协议升级、大型前端框架、系统性能或端到端延迟监控。

## Validation

1. Rust 投影测试：未知、三方向、暂停、撤销、过期和租约失效；只读列表请求前后 updated/expires/last_seen 不变；没有令牌/会话 ID/正文等敏感字段。
2. 权限测试：未登录、设备账户、客户端 Bearer 不能访问；CSRF/Origin 缺失或跨站修改失败；现有会话失效处理不回填旧数据。
3. 原生 Chromium：三个页签键盘与鼠标切换、补发表单只在设置页出现；切换/刷新保留未保存输入，保存后有审计；错误/空状态与恶意标签安全显示。
4. 1440px 与 390px、浅/深色、中文/英文检查设备信息与操作不溢出；仅在实施阶段按用户要求进行实际视觉验证。
5. 客户端回归：Omarchy、Ubuntu、Android 协议与方向 ACK 流程不变；管理页连续刷新不改变设备 TTL、方向版本或同步游标。
6. 仓库检查：`cargo fmt --all -- --check`、`cargo test --locked`、`cargo clippy --locked --all-targets --all-features -- -D warnings`、现有 admin-ui fixture。

## Stop conditions

- 如果实现需要客户端健康、剪贴板写入结果或 RTT，新开协议需求，不用服务端活动时间推断。
- 如果需要管理员远程控制客户端方向或敏感开关，先讨论权威关系与安全边界，不复用只读 DTO 作为配置下发。
- 如果聚合设置改动影响同步 hot path 或现有令牌鉴权，拆开评审与测试，不能以页面改造为由降低同步门禁。

## Design documentation

接受并实施后，同步更新英文/中文 README、方向协议管理说明和验证记录；说明“活动在线”“服务端已接受方向”“数据租约”和客户端本地健康的区别。
