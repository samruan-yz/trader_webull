# Discord → Webull Trader（Rust）

> 监听 Discord 频道的交易信号，并将其自动下达到 Webull（模拟或实盘）。
>
> 本项目基于非官方 Rust 客户端对接 Webull，仅供学习研究，请自担风控与合规责任；与 Webull/Discord 均无关联。

---

## 功能概览

* **Discord 监听（serenity‑self）**：使用用户 Token 登录（你已确认获得许可），可同时监听多个频道；对发帖人支持不区分大小写的子串模糊匹配，仅匹配到的作者消息才参与解析。
* **信号解析**：将文本信号解析为结构化的股票/期权 `TradeSignal`（含方向 BTO/STC、数量、市价/限价、限价价格等）。
* **下单执行（Webull）**：支持股票与期权；期权会从链上筛选目标合约；支持 `DAY/GTC` 等 TIF。
* **全局执行模式**：买单与卖单可分别设置为 `LIMIT` 或 `MARKET` 两种模式。

  * LIMIT 模式下，可配置两个百分比偏差：买单在信号价基础上“上浮”，卖单在信号价基础上“下调”。
  * MARKET 模式下，即使信号里给了具体价格也会当作市价单处理。
* **订单生命周期监控（非阻塞）**：

  * 下单后立即启动后台监控任务，主循环可继续处理后续信号与定时任务。
  * **买单**：监控至 `buy_timeout_sec`；若仍未完全成交，则撤单。仅已成交的数量会被加入本地持仓（加权成本）。
  * **卖单**：监控至 `sell_timeout_sec`；若仍未完全成交，撤掉剩余部分并将其改为市价单重新提交，再继续监控。
* **风控规则**：

  * 单笔名义金额上限（股票：价格×股数；期权：权利金×合约数×100）。
  * 禁止卖出未持仓：对 STC 信号检查当前持仓数量/合约数是否足够，不足则拒单。
  * Dry‑run：仅记录日志，不真实下单。
* **状态与盈亏**：

  * 本地 state 维护“完整持仓”（股票：股数与平均成本；期权：合约数与平均权利金）。
  * 每次完全或部分平仓会立即记录一条“当日已实现盈亏”条目（含日期、标的、数量与盈亏金额）。
  * 按 `flush_interval_sec` 周期与 Webull 同步持仓，确保状态与券商侧一致。
* **并发模型**：

  * 使用 Tokio 的 `current_thread` 运行时与 `spawn_local`；订单监控任务为非阻塞本地任务，避免非 `Send` future 的跨线程限制。

---

## 工作流程（简述）

1. 启动后从配置加载频道与跟踪用户，登录 Discord 与 Webull，并做一次初始持仓同步。
2. 收到匹配作者的消息后，尝试解析为 `TradeSignal`；
3. 估算名义金额并执行风控；
4. 按全局执行模式（以及 LIMIT 偏差）组装下单参数并提交至 Webull；
5. 立即在后台启动对应的监控任务：

   * 买单：超时未成则撤单；仅已成交的数量写入本地持仓（更新加权成本）。
   * 卖单：超时未成则撤单→剩余转市价→继续监控，成交后即时记已实现盈亏；
6. 主循环同时继续接收后续信号，并按固定间隔与 Webull 同步持仓。

---

## 信号格式（概念）

* 支持股票与期权，方向为 BTO/STC；整体大小写不敏感。
* 股票支持市价与限价；期权包含标的、行权价、看涨/看跌、到期（MM/DD）与价格/市价。
* 解析结果会标准化为结构化字段并进入执行与风控流程。

---

## 配置要点（概念）

* Discord：多个频道 ID、跟踪作者列表（模糊匹配）。
* Webull：区域与交易模式（paper/live）。
* 风控：单笔最大名义金额。
* 执行：

  * `buy_mode` / `sell_mode`（LIMIT 或 MARKET）
  * `buy_timeout_sec` / `sell_timeout_sec`
  * `buy_limit_slippage_pct` / `sell_limit_slippage_pct`
  * `tif`
* State：本地状态文件路径与 `flush_interval_sec`（定期持仓同步周期）。

---

## 配置使用说明

### 1) 必备环境

* **Rust**（建议稳定版，支持 `cargo`）
* **Discord 用户 Token**（你已确认获得许可）
* **Webull 账号**（`paper` 或 `live`；实盘需交易 PIN）

### 2) 环境变量（`.env`）

在项目根目录放置 `.env` 文件，至少包含：

* `DISCORD_USER_TOKEN`：Discord 用户 Token（serenity‑self 登陆所需）
* `WEBULL_USERNAME` / `WEBULL_PASSWORD`：Webull 账号与密码
* `WEBULL_TRADING_PIN`：仅在 `webull.mode = live` 时需要（6 位交易 PIN）

> 也可用系统环境变量方式提供，`.env` 仅为便捷。

### 3) `config.yaml` 关键字段（概念）

* `discord.channel_ids`：需要监听的**多个频道 ID**（字符串数组）
* `discord.tracked_users`：**作者模糊匹配**名单（子串、不区分大小写）
* `webull.region` / `webull.mode`：区域与交易模式（`paper` 或 `live`）
* `risk.max_position_value`：**单笔名义金额上限**（USD）
* `exec.dry_run`：干跑，不真实下单
* `exec.tif`：`DAY` / `GTC` 等
* `exec.buy_mode` / `exec.sell_mode`：`LIMIT` 或 `MARKET`
* `exec.buy_timeout_sec` / `exec.sell_timeout_sec`：买/卖**监控超时**（秒）
* `exec.buy_limit_slippage_pct` / `exec.sell_limit_slippage_pct`：LIMIT 模式下，买单**上浮**、卖单**下调**的百分比（例如 0.01 = 1%）
* `state.path`：本地状态文件路径（JSON）
* `state.flush_interval_sec`：**定期与 Webull 同步持仓**的间隔（秒）

> 提示：
>
> * 如果设定 `buy_mode = MARKET` / `sell_mode = MARKET`，**即便信号给出具体价格**也会按市价发单。
> * `LIMIT` 模式下若信号是市价（M），将以当时中价为基准，应用对应偏差后作为限价。

---

## 运行方式

1. **准备凭证与配置**：

   * 在项目根创建 `.env`，填入 Discord/Webull 信息；
   * 配置 `config.yaml`（建议先用 `paper` + `dry_run: true` 验证）。
2. **拉取依赖并编译**：在项目根执行 `cargo run --release --bin discord-webull-trader`；
3. **运行**：`cargo run --release`

   * 首次启动会：登录 Discord 与 Webull → 首次同步持仓 → 开始监听频道与定时同步。
   * 控制台会输出**下单/监控/同步**日志与风控拒单信息。
4. **验证**：在被监听的频道按信号格式发帖，观察程序日志；
5. **切换实盘（可选）**：将 `webull.mode` 设为 `live`，并在 `.env` 提供 `WEBULL_TRADING_PIN`。

### 退出与数据

* 程序运行期间会定期将**完整持仓**与**当日已实现盈亏**落盘到 `state.path` 对应的 JSON 文件；
* 正常退出即可（如 `Ctrl+C`），数据会在下次启动时加载；
* 若要只做行情/风控演练，保持 `dry_run: true` 即可。

---

## 版本更新记录

### v1.1（2025‑09‑13）

* Discord 监听切换为 serenity‑self；支持多频道与作者模糊匹配（不区分大小写的子串）。
* 状态重构：从“仅持仓数量”升级为“完整持仓（含平均成本）+ 每日已实现盈亏条目”。
* 风控更新：移除最大持仓条目数限制；新增“禁止卖出未持仓”。
* 执行层增强：新增买/卖全局执行模式（LIMIT/MARKET）与 LIMIT 价偏差（买上浮、卖下调）。
* 订单监控：

  * 买单超时自动撤单，仅记录实际成交部分；
  * 卖单超时撤单后将剩余改为市价并继续监控；
  * 监控为非阻塞后台任务，主循环可继续处理新信号。
* 周期同步：按 `flush_interval_sec` 定期从 Webull 拉取并刷新本地持仓。
* Webull 封装：新增 `positions_simple`、基于 `get_orders(None)` 的订单状态查询与撤单。

### v1.0（2025‑09‑12）

* 初始版本：

  * Discord 监听 + 基本信号解析（股票/期权）；
  * Webull 下单（模拟/实盘，实盘需交易 PIN）；
  * 市价/限价、名义金额风控、Dry‑run、最小 JSON 状态与日志。
