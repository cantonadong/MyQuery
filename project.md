# MyQuery 项目与开发交接说明

更新日期：2026-11-23。依据当前仓库源码、构建脚本、Git 记录及历史方案整理；当前 `Cargo.toml` 版本为 `1.7.0`。本文用于帮助后续开发者和 agent 快速接手，功能“已实现”表示代码已存在，不代表本次重新运行验证。

## 1. 项目定位与当前边界

MyQuery 是面向 PostgreSQL 和 Amazon Redshift 的 Windows 桌面 SQL 客户端，早期名称为 SQL Desk，因此包名、部分目录和内部标识仍保留 `sql-desk` / `SqlDesk`。

桌面程序负责连接管理、对象浏览、SQL 编辑和交互查询；查询核心同时以 Rust 库和 C ABI DLL 交付，供其他应用复用。它不是 BI 报表设计器，也不是独立网络查询服务。根目录 `AGENTS.md` 中部分画布、图表、过滤器规则属于 UI 约束，不能据此认定本项目已经实现这些功能。

## 2. 已实现功能

### 连接与会话

- 保存、修改、删除 PostgreSQL / Redshift 连接，配置主机、端口、数据库、用户名、密码及 TLS。
- TLS 可验证服务器证书，也提供显式信任服务器证书选项；后者会放宽证书与主机名验证，不能默认开启来掩盖连接错误。
- 持久数据库会话、TCP keepalive 和每 30 秒的连接检查；后台检测连接状态并尝试重连。
- 连接单选；数据库、schema 支持“全部”范围浏览，可配置浏览范围并调整顺序。
- 多标签查询分别保存会话状态；对象浏览与查询任务分别调度，降低长查询对浏览操作的影响。

### 数据库对象浏览

- 浏览表、视图、物化视图与字段；显示字段类型、可空性、默认值等结构信息。
- 对象名及缓存字段名搜索、按连接保存前缀筛选、隐藏与恢复对象。
- 元数据本地缓存和共享 schema 快照；保存前一次结构，标记对象或字段变化。
- 手动刷新与后台预取；缓存允许先显示已保存结构，不能等同于数据库实时状态。
- 复制对象名、字段名、表结构，以及生成快速取数、分组查询 SQL。

### SQL 编辑与文档

- 多 SQL 标签，新建、切换、重命名、关闭；未保存内容提示和退出处理。
- SQL 文件导入、保存、另存，以及标签文本与活动标签恢复。
- SQL 语法高亮、查找、自动缩进、引号配对，以及结合表名、字段和别名的补全。
- 每个标签保留查询结果视图和有限历史；历史最多 20 条。结果和历史属于运行时状态，不随标签恢复文件持久化。

### 查询执行

- 执行 SQL、多语句、多结果集；显示耗时、接收行数、执行阶段、错误详情及无行集命令的影响行数。
- 查询取消、可设置超时；默认超时为 3600 秒，设置界面使用分钟。
- 连接必须单选具体连接，不提供“全部连接”；SQL 原样交给当前连接执行。
- 已移除跨连接语句路由和连接名前缀补全；保留单连接内的多语句、多结果集、取消和超时。
- 切换连接优先显示缓存、复用有效浏览会话，后台按需建连；SQL 标签记录并恢复最近成功查询的连接、库和模式，允许手动切换。
- 完全相同的结果集可合并并显示重复次数；结果标签也可由 SQL 标签解析逻辑生成。

### 结果与交互

- 查询结果流式写入临时 JSONL 文件，每 100 行记录分页偏移，界面按页加载。
- 多结果集切换和关闭、列宽调整、列适配、行/列/单元格选择、双击复制单元格。
- 复制当前页、全部结果或选中内容，可选择是否包含表头；支持 CSV 全量导出。
- 对完整结果按列排序；数字排序通过十进制字符串构造排序键，避免先转浮点数丢失精度。
- `Cells = Vec<Option<String>>` 区分数据库 NULL 与普通字符串，源值不因界面显示而被数值转换；CSV 将 NULL 输出为字面值 `NULL`，因此该导出形式无法无歧义地区分 NULL 和文本 `NULL`。
- 简体中文 / 英文切换、跟随系统语言的初始选择、明暗主题、窗口最大化状态保存、紧凑控件和可拖动面板分隔线。

## 3. 技术栈

| 范围 | 当前实现 |
| --- | --- |
| 语言与构建 | Rust 2024 edition、Cargo、PowerShell、GNU Windows 工具链与 LLVM-MinGW |
| 桌面 UI | Slint 1.15、winit 后端、software renderer、声明式 `.slint` 组件 |
| 异步与数据库 | Tokio、tokio-postgres 0.7、futures-util，使用 PostgreSQL 协议访问 PostgreSQL / Redshift |
| TLS | native-tls、postgres-native-tls |
| 配置与结果序列化 | serde、serde_json；临时 JSONL 结果和 JSON 配置 |
| 文件与系统交互 | tempfile、csv、rfd 原生文件对话框、arboard 剪贴板、sys-locale |
| Windows 能力 | windows-sys：DPAPI、窗口与输入、资源及系统接口；keyring 用于旧密码迁移 |
| 跨语言复用 | `rlib` + `cdylib`，C ABI 接收和返回 JSON |

版本约束以 `Cargo.toml` 为准，实际锁定依赖以 `Cargo.lock` 为准。无需 Node.js、浏览器前端或单独部署后端服务。

## 4. 架构与数据流

```text
Slint UI / SQL 文档状态（UI 线程）
    │ callback → Request / Command
    ▼
有界队列（容量 64）→ sql-worker 线程 / Tokio current-thread runtime
    ├─ 对象浏览、缓存读取与刷新
    ├─ 按文档管理查询任务、会话与结果历史
    └─ 当前单选连接 → db → PostgreSQL / Redshift
                                      │
                                      ▼
                         ResultSet 临时 JSONL + 分页索引
                                      │
                         分页 / 复制 / 排序 / CSV 导出
                                      │
                         Slint 事件循环回传 UI 更新

其他调用方 → C ABI / JSON → 同一套 db / results → JSONL 输出文件
```

UI 使用 `Rc<RefCell<...>>` 与 Slint model 管理本地状态；后台通过 weak handle 和 `upgrade_in_event_loop` 回传更新。查询返回时带文档身份，避免用户切换标签后结果写进错误标签。不要从后台任务直接操作 UI，也不要把整份结果加载进表格 model。

### 模块导航

| 文件 | 职责与修改入口 |
| --- | --- |
| `src/main.rs` | 程序入口、`Command` / `Request`、Worker、后台调度、会话复用与单连接执行、UI 回调、错误本地化 |
| `src/lib.rs` | `myquery_core` 导出边界，公开 config / db / results，纳入 FFI |
| `src/db.rs` | `Session`、连接与 TLS、心跳、元数据 SQL、流式执行、结果上限、`QueryFailure` 与错误详情 |
| `src/results.rs` | `Cells` / `ResultSet`、JSONL 临时文件、分页、排序、复制与 CSV / JSONL 导出 |
| `src/config.rs` | `Profile` / `Settings` / `BrowserSelection`、共享配置合并、文件锁、密码加密与迁移 |
| `src/cache.rs` | `MetadataCache`、共享 `ScopeMetadata`、缓存身份、结构差异、快照文件及锁 |
| `src/documents.rs` | `Documents`、标签文本和路径、脏状态、保存与关闭、恢复文件、结果视图与历史；`bind` 接回调 |
| `src/syntax.rs` | `tokens`、高亮、SQL 结果标签解析 |
| `src/completion.rs` | 补全候选、别名与表解析、`insert` 替换范围 |
| `src/dialogs.rs` | Windows 原生对话框及窗口关联 |
| `src/ffi.rs` | C ABI 请求、只读查询策略、超时和输出内存生命周期 |
| `ui/main.slint` | 主窗口、面板、菜单、状态、交互与 UI 属性/回调；包含共享 I18n |
| `ui/sql-editor.slint` | SQL 编辑器组件及编辑交互 |
| `ui/controls.slint` / `ui/dense-widgets.slint` / `ui/theme.slint` | 公共控件、紧凑列表表格及主题；新增 UI 优先复用 |
| `build.ps1` / `build.rs` / `app.rc` | 发布构建、Slint 编译、Windows 图标资源和根目录产物覆盖 |

### Rust 库与 DLL 的区别

桌面 EXE 直接使用 `myquery_core` Rust 库，并不需要通过根目录 DLL 执行查询。EXE 与 DLL 包含同源核心机器码，修复共享逻辑后仍需更新相应调用方产物。

DLL 当前导出 `myquery_core_version`（返回 ABI 版本 1）、`myquery_core_execute`、`myquery_core_free`。调用方传入 UTF-8 JSON C 字符串；请求包括 profile、password、sql、output、timeout_seconds、max_rows、max_bytes，可选 parameters 和 typed。返回 JSON 包含 headers、oids、row_count 或 error，数据行写入请求指定的 JSONL 文件。返回指针必须用 `myquery_core_free` 释放，不能由调用方使用其他分配器释放。

FFI 每次调用创建 runtime 与连接，在只读事务中执行，限制单个有效结果集，并可返回列类型 OID。`parameters` 当前是按 `%s` 分割并用数据库字面量转义后拼接，不是驱动原生参数绑定；修改时需要同时考虑字符串/注释中占位符的歧义及外部调用方约定。FFI 的只读和单结果集策略不能套到桌面交互查询上。

## 5. 配置、缓存与数据安全

| 路径 | 用途 |
| --- | --- |
| `%APPDATA%\MyDataTools\connections.json` | 共享连接、浏览范围及设置；支持从旧 `%APPDATA%\SqlDesk\connections.json` 迁移 |
| `%APPDATA%\MyDataTools\passwords.dat` | Windows DPAPI 加密密码；同目录有锁和迁移标记 |
| `%APPDATA%\SqlDesk\metadata-cache.json` | 旧式/兼容元数据缓存，仍有代码使用 |
| `%LOCALAPPDATA%\MyDataTools\metadata\*.json` | 按引擎、主机、端口、用户、数据库、schema 隔离的共享快照及锁 |
| `%LOCALAPPDATA%\MyQuery\tabs.json` | SQL 标签恢复文件，包含 SQL 文本，不包含查询结果和历史 |
| 系统临时文件 | 活跃 ResultSet 的 JSONL 数据，随资源释放清理 |

配置和密码写入采用同目录临时文件与替换机制；共享配置保存会加锁，读取最新文件并按加载基线合并改动，保留未知字段。不要用旧内存配置整文件覆盖磁盘，更不要删除 `extra` / `shared_id` 等兼容字段。

密码从旧 Windows Credential Manager、旧 EXE 同目录加密文件迁移到共享 DPAPI 文件。DPAPI 默认与 Windows 用户环境绑定，复制 `passwords.dat` 到另一台机器或另一账户不等于可以解密。不要把真实密码、连接配置、恢复 SQL 或结果文件提交到仓库或日志。

### 当前资源上限

| 对象 | 限制 |
| --- | --- |
| SQL 导入/执行 | 2 MiB |
| 单次查询结果存储 | 512 MiB |
| 单行 | 原始内容及 JSON 编码行均受 4 MiB 限制 |
| 结果宽度/数量 | 最多 256 列、32 个结果集 |
| 显示分页 | 100 行/页；页预览预算 2 MiB |
| 剪贴板 | 8 MiB |
| 单次元数据读取 | 20,000 行、8 MiB，另有单行限制 |
| 连接配置 / 加密密码文件 | 各有 1 MiB 检查 |
| 兼容元数据缓存 | 8 MiB、128 条目；不要据此推断全部共享快照的总大小上限 |

磁盘分页不代表所有操作都为常量内存：排序仍收集每行的排序列与文件位置，重建结果文件，较大结果会增加内存与磁盘开销。

## 6. 已遇问题与维护经验

以下依据历史记录与当前防护代码整理；没有完整历史证据的事项只作为维护注意点，不认定已复现或彻底解决。

| 问题/坑 | 当前处理与后续注意 |
| --- | --- |
| 没有 MSVC linker / SDK 导致 Windows 构建受阻 | 使用 GNU Rust + LLVM-MinGW；构建依赖 `ld.lld.exe` 和 `x86_64-w64-mingw32-windres`，只安装 Rust 不够 |
| Slint 菜单与父容器尺寸依赖循环 | 历史记录显示已修复；调整菜单/布局要检查编译警告与实际拖动行为 |
| 浏览器选中的数据库/schema 与真实会话不一致，出现 relation-not-found | 切换时同步上下文，执行前准备匹配会话与 search_path，失败时清理陈旧状态；原 Redshift 具体 SQL 的数据库验证在历史记录中未完成 |
| 将普通 SQL 错误也当成断线 | 区分可恢复数据库错误与连接/协议错误；不能因语法错误一律断开，更不能自动重试可能已产生副作用的 SQL |
| 取消、超时或结果超限后协议未消费完 | 发送取消并按错误类型使会话失效，避免残留响应污染下次查询；事务副作用不能靠 UI 状态推断 |
| 长查询完成时用户已切换或关闭标签 | 按文档身份回传、分别保存会话与结果视图，修改回调必须保留身份校验 |
| 多窗口或其他应用共同保存连接导致设置丢失 | 文件锁、基线合并、保留未知字段及原子替换；连接改名还涉及密码、范围、缓存和当前会话引用 |
| 密码方案发生迁移 | 以当前 DPAPI 文件为准；早期文档写“Credential Manager 存密码”已不足以描述现状，保留迁移兼容与锁 |
| 缓存策略多次演进 | 历史出现过 7 天、按自然日刷新方案，当前读取可直接使用已保存缓存；以 `cache.rs` 和调度刷新代码为准，不承诺严格每日失效 |
| 恢复文件损坏被默认空状态覆盖 | 加载失败时禁用持久化并保留原文件；不要吞掉错误后无条件自动保存 |
| 大结果拖慢 UI 或耗尽内存 | 流式落盘、分页、大小限制、进度节流；导出读全量文件，不能只导出界面当前页 |
| NULL、长数字与展示截断损失数据含义 | 保留 `Option<String>` 原值，复制与导出走底层数据；不要把预览文本回写为源值 |
| Windows 正在使用 EXE 导致覆盖失败 | 终止现有项目进程，覆盖同一个 `MyQuery.exe`；不要生成越来越多 preview/test EXE |
| SimpleWall 新规则对现有进程无效 | 新放行某可执行文件后，必须完全终止该程序所有实例再启动，再验证网络 |

## 7. 构建与交付

工作目录为仓库根目录。需要已安装的 `stable-x86_64-pc-windows-gnu` 工具链及 LLVM-MinGW，使用既有构建入口：

```powershell
.\build.ps1 -LlvmBin '实际的 LLVM-MinGW bin 目录'
# 或提前设置 LLVM_MINGW_BIN，然后执行：
.\build.ps1
# 仅更新共享库：
.\build.ps1 -CoreOnly
```

脚本设置 linker、Rust flags 和 Slint style，执行 release / locked 构建；按 PNG 更新多尺寸 ICO；复制 DLL 到根目录，完整构建还会终止已有项目进程并将 `target/release/sql-desk.exe` 覆盖为根目录 `MyQuery.exe`。脚本本身不负责启动应用。`CoreOnly` 仍受当前包的 Slint/Windows 构建依赖约束，尚非完全独立的跨平台核心构建。

用户测试固定入口：`D:\Dev\MyQuery\MyQuery.exe`。修改可运行程序后，按用户要求构建并重启这一入口。只改文档无需构建或启动。

## 8. 后续 agent 接手规则

1. 先读根目录 `AGENTS.md` 与本文，再读任务直接涉及的模块；用户新指令优先于历史方案。始终直接在 `main` 工作，不创建或使用其他分支。
2. 未经用户明确要求，不运行测试；需要可运行交付时构建并报告结果，不将“编译通过”写成“数据库/交互验证通过”。已有 `#[cfg(test)]` 可供理解边界，但本次未执行。
3. 先检查工作区改动，保留用户未提交内容；Git 元数据写入使用获批执行，不修改 NTFS ACL 绕过保护。不要因为本文存在构建产物说明就自动提交或发布。
4. GitHub 凭据优先复用正常 Windows 用户的 keyring；沙箱账户显示无效 token 不代表需要重新登录。真正变更认证后执行 `gh auth setup-git` 和 `gh auth status`。
5. 改 UI 时复用主题与公共控件，保持紧凑按钮、相同行为等宽对齐；提示共 3 秒且消失前淡出。新增或修改文案接入共享 I18n，用户 SQL、字段名和数据不翻译。
6. 修改 UI 属性/回调时同时检查 `.slint` 声明、Rust `on_*` 绑定、文档状态捕获/恢复和后台回传；只改一端通常不完整。
7. 修改数据库执行时检查桌面 Worker 与 FFI 两条调用路径、取消、超时、事务与资源上限；修改结果存储时检查分页、复制、排序、导出、重复结果比较。
8. 修改连接字段/标识时检查序列化兼容、配置合并、密码键、范围配置、缓存身份和已保存会话；修改补全时检查 UTF-8 字节偏移与引号名称。
9. 共享配置/快照可能被其他应用使用；MyBI 源码不在本仓库，不要仅凭这里的 DLL 接口就声称外部集成验证完成。
10. 交接时写清改动、构建与运行情况、没验证的行为及剩余风险，不泄露真实连接或 SQL 数据。

## 9. 长期方向与未完成事项

详见 [共享查询核心长期方案](long-term-plans/shared-query-core.md)。已经存在同源 Rust 库和 DLL，以及只读查询接口；以下仍属演进方向，不能作为完成项宣传：

- 将查询核心拆为独立项目/包，解除 UI、Windows 和本地凭据依赖。
- 统一核心版本发布与调用方 ABI 兼容检查；ABI 版本与应用版本分别维护。
- 构建和验证 Linux `.so`、服务器平台适配。
- 根据实际负载改进有上限的连接复用、分批读取及更完整的外部取消接口。
- MyBI 后台进程加载核心并发布快照的后续架构；其外部接入状态需在 MyBI 仓库确认。

当前不支持的能力还包括跨连接 JOIN，以及本仓库未实现的 IAM/SSO 登录流程。真实数据库兼容性、长时间连接、断网恢复、大结果排序和服务器性能需要对应环境验证。

## 10. 参考与本次核对范围

- 当前事实入口：`Cargo.toml`、`build.ps1`、`build.rs`、`src/`、`ui/`。
- 历史设计记录：`docs/superpowers/plans/`。其中旧程序名、缓存周期、密码存储方式和“不启动应用”的历史指令可能过时，不能覆盖当前代码与用户规则。
- 长期架构结论：`long-term-plans/shared-query-core.md`。
- 本次仅进行源码、文档和 Git 历史检查并新增本文；未运行测试、构建、应用、数据库查询或外部 MyBI 集成验证。

## 11. 1.6.0 开发记录（2026-11-18）

- 已完成：对象右键“全选”前新增“查看定义”；立即打开只读标签，显示加载状态，失败显示错误，关闭不提示保存，回传按标签 ID 匹配。
- 已完成：SQL 标签恢复最近成功查询的连接、库和模式；缓存优先浏览、有效会话复用，切换完成后统一更新对象列表，减少清空重建和文本闪动。
- 已完成：结构变化检测改为按对象分组，避免大模式逐表扫描完整字段快照。
- 已完成：SQL 高亮片段按行绘制，行位置从原生编辑器测量；高亮模型和行位置按标签缓存，内容或缩放变化时重新计算。
- 已完成：移除跨连接路由、连接“全部”选项及相关前缀补全；查询原样提交当前单选连接。
- 验证：构建 Windows x64 Release 并交付同一路径 MyQuery.exe；按用户要求不运行测试。数据库兼容性、长 SQL 选区对齐及性能仍需用户实际验证，不作为已实测通过宣传。
- 发布：1.6.0 便携包仅包含 MyQuery.exe 和 myquery_core.dll，不包含凭据、配置、缓存或恢复的 SQL 内容。
## 12. 1.7.0 开发记录（2026-11-23）

- 已完成：移除顶部菜单栏，设置菜单改为“连接”下拉右侧的齿轮按钮（连接配置、浏览范围、查询超时、主题、语言）。
- 已完成：结果区“复制”按钮移到按钮列最下方。
- 验证：构建 Windows x64 Release 并重启 MyQuery.exe；按用户要求不运行测试。设置弹出菜单需用户实际验证。
- 发布：1.7.0 便携包仅包含 MyQuery.exe 和 myquery_core.dll。
