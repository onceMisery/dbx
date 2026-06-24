# 查询结果流式导出设计方案

日期：2026-06-24

状态：设计方案（待实现）

## 1. 背景

当前 SQL 编辑器查询结果导出在 `context="results"` 下仍走内存路径：

1. 前端通过 `queryStore.fetchTabResultForExport` 分页获取结果。
2. 每页结果在 JS 中通过 `rows.push(...)` 累积成完整 `QueryResult`。
3. CSV/XLSX 再一次性传给 Rust 命令落盘。

这会带来两个问题：

1. 导出大结果集时，前端内存随总行数线性增长，且 IPC 需要传输完整结果集。
2. 为防止 OOM，导出必须保留行数上限，但现有 `exportBatchSize` 被混用为分页批大小和导出上限，语义不清。

表数据导出已经有后端流式路径：Rust 内部分页，每批增量写盘，Tauri 通过事件回传进度，Web 通过 SSE 和临时文件下载。查询结果导出应采用同类机制，但必须额外保证“导出结果等价于用户 SQL 的结果语义”。

### 1.1 代码基线与 review 结论

当前代码基线显示，查询结果导出和表数据导出的能力边界不一致：

1. `apps/desktop/src/stores/queryStore.ts` 的 `fetchTabResultForExport` 会把分页结果累积到一个完整 `QueryResult`，这是查询结果 CSV/XLSX 导出内存压力的根因。
2. `apps/desktop/src/composables/useDataGridExport.ts` 已有 `exportFullTableDataViaBackend`，表数据导出已经具备后端流式、进度和取消路径，查询结果导出应复用同类架构。
3. `crates/dbx-core/src/query_result_sql.rs` 的 `build_query_pagination_execution_plan` 在 `use_agent_cursor && offset == 0` 时会让首页执行 `query_base_sql`，这对查询结果导出有语义风险，因为用户排序后的 `resultSortedSql` 可能被绕开。
4. agent 侧 `maxRows` 是 Java `int`，无上限导出不能传 `usize::MAX`，应使用 `i32::MAX` 或显式 agent 协议语义。
5. `exportBatchSize` 目前容易被误解为总行数上限，应拆成“每批读取行数”和“总导出上限”两个设置。
6. 查询结果 CSV 的 `NULL` 语义是 `"NULL"`，不能复用表数据导出的空字符串语义。

因此 review 结论是：流式导出方案合理，但导出 SQL 必须以当前结果语义为 owner；性能优化只能作为可证明安全的执行策略，不能改写用户实际想导出的结果。

## 2. 目标

本方案目标：

1. 查询结果 CSV/XLSX 导出从前端全量累积改为后端流式分页写盘。
2. 默认总行数上限为 `100000`，用户可在设置中关闭上限。
3. 导出过程可取消，取消后清理部分文件并释放查询 session。
4. 保持现有查询结果 CSV 语义，包括 `NULL` 导出为 `"NULL"`。
5. 增加安全的 keyset 优化能力，仅在静态分析能证明不会改变查询语义时启用。
6. JSON、Markdown、SQL 导出本期不改，继续走旧内存路径和上限保护。

非目标：

1. 本期不实现 JSON/Markdown/SQL 的流式导出。
2. 本期不对任意 JOIN 查询做 SQL 改写式 keyset 分页。
3. 本期不实现 XLSX 自动多 sheet 拆分。
4. 本期不改变表数据导出的现有行为。

## 3. 第一原则与不可破坏约束

### 3.1 第一原则

查询结果导出必须导出“当前查询结果语义下的数据”。如果用户已经排序、过滤、分页或执行了复杂 SELECT，导出路径不能为了性能改变结果集合或顺序。

### 3.2 不可破坏约束

1. 导出 SQL 以当前结果对应的实际 SQL 为准，优先使用 `resultSortedSql`。
2. `queryBaseSql` 只能用于计数、分析和 fallback，不得覆盖实际导出 SQL。
3. keyset 优化必须是可证明安全的优化，不能成为语义 owner。
4. 任意不确定的 SQL 分析结果都必须回退到普通分页或 agent session。
5. 取消、错误、行数上限命中时都必须释放 agent query session 和 client session。

### 3.3 关于默认表 id 游标优化的结论

“默认带上表 id 做游标查询”作为性能优化方向是合理的，但不能作为无条件默认执行路径。合理落地方式是：

1. 设置项 `queryExportKeysetOptimizationEnabled` 默认开启，表示“允许优化器尝试 keyset”，不是强制改写所有查询。
2. 只有静态分析确认查询是安全单表查询，并且能找到稳定主键或唯一键时，才可用表 id 或主键列做游标。
3. 对 JOIN、聚合、`DISTINCT`、`UNION`、窗口函数、复杂投影、用户显式排序但无法稳定 tie-break 的查询，必须自动回退。
4. 回退路径必须继续使用用户定义的排序规则，即使用 `resultSortedSql` 或等价 SQL 做 agent/session 或 LIMIT/OFFSET 分页。
5. 不允许对 JOIN 查询猜测“主表 id”作为游标，因为 JOIN 后一行结果不一定对应主表一行，可能导致漏行、重复行或顺序变化。

结论：配置默认开启是可接受的，前提是默认开启的是“安全优化尝试”，而不是“默认改写 SQL”。优化器无法证明安全时，必须保持用户 SQL 语义优先。

## 4. 总体方案

查询结果导出新增后端流式核心 `query_result_export`，由前端只传导出上下文和目标路径，后端负责分页、写盘、进度和取消。

优先级：

1. Agent/session 分页。
2. 安全 keyset 优化。
3. LIMIT/OFFSET 或数据库方言分页。
4. 无法安全分页时返回明确错误，不回退到前端全量导出。

CSV 和 XLSX 共享同一个分页循环，只在写盘器不同。

执行策略选择规则：

1. 如果连接支持 agent/session 游标，优先使用 agent/session 读取同一个结果集，避免重新执行带 offset 的 SQL。
2. 如果不走 agent/session，且 keyset 配置开启并通过安全分析，则使用 keyset 分页。
3. 如果 keyset 未命中，则保留用户排序后的实际 SQL，使用数据库方言分页。
4. 如果无法生成安全分页 SQL，则报错，不为了兼容回到前端全量累积。

## 5. 配置设计

新增或修订编辑器设置：

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `exportBatchSize` | `number` | `10000` | 每批读取行数，只表示分页批大小 |
| `exportRowLimitEnabled` | `boolean` | `true` | 是否启用查询结果导出总行数上限 |
| `exportRowLimit` | `number` | `100000` | 查询结果导出总行数上限 |
| `queryExportKeysetOptimizationEnabled` | `boolean` | `true` | 是否允许对安全单表查询启用 keyset 优化 |

设置 UI 文案要求：

1. `exportBatchSize` 文案说明为“每批读取行数”，不得再说明为总上限。
2. `exportRowLimit` 文案说明为“查询结果导出最多行数”。
3. 关闭 `exportRowLimitEnabled` 后，CSV 可无限制流式导出直到结果结束。
4. XLSX 即使关闭总上限，仍受 Excel 行数上限约束。
5. `queryExportKeysetOptimizationEnabled` 文案说明为“仅对可安全识别的单表查询生效，复杂查询自动回退”。

## 6. Rust Core 设计

新增文件：

```text
crates/dbx-core/src/query_result_export.rs
```

新增请求结构：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryResultExportRequest {
    pub export_id: String,
    pub connection_id: String,
    pub database: String,
    pub schema: Option<String>,
    pub sql: String,
    pub query_base_sql: String,
    pub database_type: DatabaseType,
    pub use_agent_cursor: bool,
    pub file_path: String,
    pub format: String,
    pub page_size: usize,
    pub row_limit: Option<usize>,
    pub total_rows: Option<u64>,
    pub timeout_secs: Option<u64>,
    pub keyset_optimization_enabled: bool,
    pub client_session_id: Option<String>,
    pub execution_id: Option<String>,
}
```

进度结构建议复用 `TableExportProgress` 字段形态，`table_name` 使用空串或 `"Query Result"`：

```rust
pub struct QueryResultExportProgress {
    pub export_id: String,
    pub rows_exported: u64,
    pub total_rows: Option<u64>,
    pub status: ExportStatus,
    pub error_message: Option<String>,
}
```

如果为了减少前端改动复用 `TableExportProgress`，需要在文档和代码中明确 `table_name` 对查询结果无业务含义。

## 7. 分页策略设计

### 7.1 Agent/session 分页

适用条件：

1. `use_agent_cursor=true`。
2. 数据库连接走 JDBC 或 driverManagement agent。

执行规则：

1. 首页必须执行 `request.sql`，不能执行 `query_base_sql`。
2. 后续页使用 `result_session_id` 调 `fetchQueryPage`。
3. `row_limit=Some(n)` 时，agent `maxRows=n`。
4. `row_limit=None` 时，agent `maxRows=i32::MAX as usize`，避免 Java `int` 解析溢出。
5. `pageSize` 和 `fetchSize` 使用 `min(page_size, remaining_limit)`。
6. finally 中调用 `close_query_session` 和 `close_client_connection_session`。

需要修正现有分页计划的一个风险：

当前 `build_query_pagination_execution_plan` 在 `use_agent_cursor && offset == 0` 时会将 `sql_to_execute` 设置为 `query_base_sql`。查询结果导出不能照搬这个行为，否则用户排序后的 `resultSortedSql` 可能被导出路径忽略。查询导出应新增导出专用 plan，或者给现有 plan 增加“首页使用实际 SQL”的参数。

### 7.2 安全 keyset 优化

配置项 `queryExportKeysetOptimizationEnabled` 默认开启，但优化只在静态分析确认安全时生效。

命中条件：

1. 单条 SELECT。
2. 单表来源。
3. 能解析出 schema、table 和主键或唯一键。
4. 无 `JOIN`。
5. 无 `GROUP BY`。
6. 无 `DISTINCT`。
7. 无 `UNION`、`INTERSECT`、`EXCEPT`。
8. 无聚合函数。
9. 无窗口函数。
10. 无会破坏主键可用性的复杂投影。

排序规则：

1. 用户没有显式排序时，可使用主键升序 keyset：`WHERE pk > last_pk ORDER BY pk ASC LIMIT n`。
2. 用户有显式排序时，默认不启用 keyset。
3. 只有当排序字段和主键能构造稳定且等价的 keyset 条件时，才允许追加主键作为 tie-breaker。
4. 任意不确定时回退到普通分页。

多列主键使用词典序条件：

```sql
WHERE
  (pk1 > :last_pk1)
  OR (pk1 = :last_pk1 AND pk2 > :last_pk2)
```

安全性要求：

1. keyset 优化不得改变用户可见结果顺序。
2. keyset 优化不得漏行或重复行。
3. 对 JOIN 查询绝不尝试选择某个“主表 id”作为默认游标。

原因：JOIN 后一行结果不一定对应单表一行，一对多、多对多、聚合和去重都会让“表 id 游标”失去等价性。

### 7.3 LIMIT/OFFSET 分页

适用条件：

1. SQL 可包装为派生表。
2. 数据库方言支持对应分页语法。
3. 不走 agent/session。
4. keyset 未命中或未启用。

每页 SQL 由 `build_query_pagination_execution_plan` 或导出专用 plan 生成。

每页大小：

```text
effective_page_size = min(page_size, remaining_limit)
```

停止条件：

1. 本页返回 0 行。
2. 本页返回行数小于 `effective_page_size`。
3. `rows_exported >= row_limit`。
4. 收到取消标记。

### 7.4 不支持场景

如果 SQL 无法安全分页，并且不具备 agent/session 分页能力，后端返回明确错误：

```text
当前查询暂不支持流式导出，请简化查询或使用受支持的驱动。
```

本期不回退到旧的 CSV/XLSX 前端全量导出，因为这样会重新引入 OOM 风险。JSON/Markdown/SQL 仍保留旧路径。

## 8. 写盘格式设计

### 8.1 CSV

规则：

1. 写 UTF-8 BOM。
2. 第一批写 header 和 rows。
3. 后续批只写 rows，批之间补换行。
4. 查询结果 `NULL` 写为 `"NULL"`。
5. 字符串、数字、布尔沿用现有 `format_query_result_csv` 语义。

需要新增函数：

```rust
pub fn format_query_result_csv_rows(rows: &[Vec<Value>]) -> String
```

该函数只格式化 rows，不写 header，并复用 `value_to_query_result_csv_text`。

### 8.2 XLSX

规则：

1. 复用 `StreamingXlsxWriter`。
2. 第一行写 header。
3. 后续逐行 `write_row`。
4. finish 后显式 flush。

行数限制：

1. Excel 单 sheet 最大行数为 `1_048_576`。
2. 扣除 header 后，最大数据行数为 `1_048_575`。
3. 如果 `row_limit=None` 且格式为 XLSX，应使用 `1_048_575` 作为硬上限。
4. 如果实际结果超过硬上限，本期建议返回错误，不自动拆分多 sheet。

错误文案：

```text
XLSX 最多支持 1,048,575 行数据，请改用 CSV 导出完整结果。
```

## 9. 取消与资源清理

复用现有 `EXPORT_CANCELLED`：

1. Tauri 和 Web cancel API 都调用 `set_export_cancelled(export_id)`。
2. core 循环在每页开始前检查取消。
3. core 循环在每页写完后再次检查取消。
4. 命中取消时发 `Cancelled` 进度并停止。

清理要求：

1. 成功、错误、取消都必须调用 `clear_export_cancelled(export_id)`。
2. agent/session 路径必须关闭 query session。
3. 使用 `client_session_id` 的路径必须关闭 client session pool。
4. Web 路由在错误或取消时删除 temp file 并移除 `export_files`。
5. Tauri 命令在错误或取消时删除目标文件，避免留下半成品。

## 10. Tauri 命令设计

新增文件：

```text
src-tauri/src/commands/query_result_export.rs
```

新增命令：

```rust
#[tauri::command]
pub async fn start_query_result_export(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    request: QueryResultExportRequest,
) -> Result<(), String>
```

事件：

```text
query-result-export-progress
```

取消命令可复用：

```text
cancel_table_export(exportId)
```

但前端 API 建议暴露语义化方法：

```ts
cancelQueryResultExport(exportId)
```

内部可以转调同一个后端 cancel set。

## 11. Web 路由设计

新增文件：

```text
crates/dbx-web/src/routes/query_result_export.rs
```

新增路由：

```text
POST /api/export/query-result
GET  /api/export/query-result/progress/{exportId}
GET  /api/export/query-result/download/{exportId}
POST /api/export/query-result/cancel
```

实现复用表导出的模式：

1. `start` 创建 temp file。
2. `export_files` 存储 `exportId -> (file_path, format)`。
3. SSE channel 推送进度。
4. Done 后前端触发 download。
5. download 后删除 temp file。

## 12. 前端 API 设计

### 12.1 类型

在 `apps/desktop/src/lib/tauri.ts` 增加：

```ts
export interface QueryResultExportRequest {
  exportId: string;
  connectionId: string;
  database: string;
  schema?: string;
  sql: string;
  queryBaseSql: string;
  databaseType: DatabaseType;
  useAgentCursor: boolean;
  filePath: string;
  format: "csv" | "xlsx";
  pageSize: number;
  rowLimit?: number | null;
  totalRows?: number | null;
  timeoutSecs?: number;
  keysetOptimizationEnabled: boolean;
  clientSessionId?: string;
  executionId?: string;
}
```

新增 API：

```ts
startQueryResultExport(request, onProgress)
cancelQueryResultExport(exportId)
```

### 12.2 queryStore

新增函数：

```ts
buildQueryResultExportRequest(tabId, options)
```

职责：

1. 找到 tab 和 connection。
2. 确保连接可用。
3. 组装实际导出 SQL：

```ts
const sql = tab.resultSortedSql ?? tab.resultBaseSql ?? tab.lastExecutedSql ?? tab.sql;
const queryBaseSql = tab.resultBaseSql ?? sql;
```

4. 组装 `databaseType`、`useAgentCursor`、`timeoutSecs`。
5. 从 settings 读取 `exportBatchSize`、`exportRowLimitEnabled`、`exportRowLimit`、`queryExportKeysetOptimizationEnabled`。
6. 返回 `QueryResultExportRequest`。

`fetchTabResultForExport` 保留，继续服务 JSON/Markdown/SQL 和其他旧路径。

### 12.3 useDataGridExport

修改 CSV/XLSX 导出路径：

1. `context.value === "results"`。
2. 非选区导出，即 `!rowIds?.length`。
3. 格式为 CSV 或 XLSX。
4. 满足以上条件时调用 `exportQueryResultViaBackend(format)`。

保留旧路径：

1. 选区导出。
2. 当前页导出。
3. JSON/Markdown/SQL。
4. 多结果集 XLSX 一次性导出。

`exportQueryResultViaBackend` 行为：

1. 弹保存框。
2. 生成 `exportId`。
3. 打开进度 dialog。
4. 调 `api.startQueryResultExport`。
5. 按 progress 更新状态。
6. 取消按钮调用 `cancelQueryResultExport(exportId)`。

## 13. 兼容边界

必须保持：

1. 现有查询执行、分页、排序行为不变。
2. 表数据导出行为不变。
3. JSON/Markdown/SQL 查询结果导出行为不变。
4. 查询结果 CSV 中 `NULL` 仍为 `"NULL"`。
5. 不改变用户 SQL 结果语义。

可以改变：

1. CSV/XLSX 全量查询结果导出不再通过 JS 全量 rows 返回。
2. CSV/XLSX 对无法安全分页的查询可返回明确错误。
3. XLSX 超过格式行数上限可返回明确错误。

## 14. 风险与处理

| 风险 | 影响 | 处理 |
| --- | --- | --- |
| `exportBatchSize` 和总上限混用 | 默认上限和批大小互相污染 | 新增 `exportRowLimit`，拆开语义 |
| agent 首页使用 `queryBaseSql` | 导出忽略用户排序 | 查询导出首页必须执行 `sql` |
| `usize::MAX` 传给 Java agent | Java int 溢出 | 无上限时传 `i32::MAX` |
| JOIN 查询误用主键游标 | 漏行、重复、顺序错误 | keyset 仅限安全单表查询 |
| XLSX 无限导出 | 文件无效或超出 Excel 限制 | 单 sheet 硬上限，超过报错 |
| 取消后 session 泄漏 | agent 游标滞留 | finally 关闭 query session 和 client session |
| 桌面端取消留下半成品 | 用户误用不完整文件 | Tauri 错误或取消时删除目标文件 |

## 15. 测试计划

### 15.1 Rust 单测

覆盖：

1. CSV 多页导出。
2. `row_limit=Some(n)` 精确停止。
3. `row_limit=None` CSV 全量导出。
4. CSV `NULL` 导出为 `"NULL"`。
5. XLSX 流式写入后用 calamine 读回。
6. XLSX 超过行数上限返回错误。
7. 取消后发 `Cancelled` 进度。
8. agent 无上限参数为 `i32::MAX`。

### 15.2 分页策略单测

覆盖：

1. 单表、无排序、安全主键命中 keyset。
2. 单表、有显式排序但无法安全 keyset 时回退。
3. JOIN 回退。
4. GROUP BY 回退。
5. DISTINCT 回退。
6. UNION 回退。
7. CTE 无法证明安全时回退。
8. LIMIT/OFFSET 最后一页使用 `remaining_limit`。

### 15.3 前端测试

覆盖：

1. `context="results"` 的 CSV/XLSX 非选区导出调用 `startQueryResultExport`。
2. request 带正确 `sql`、`queryBaseSql`、`pageSize`、`rowLimit`。
3. `exportRowLimitEnabled=false` 时 `rowLimit=null`。
4. 选区导出仍走旧路径。
5. JSON/Markdown/SQL 仍走旧路径。
6. 取消按钮调用 `cancelQueryResultExport`。

### 15.4 手工验证

覆盖：

1. MySQL 大结果集 CSV/XLSX。
2. PostgreSQL 大结果集 CSV/XLSX。
3. JDBC/agent 大结果集 CSV/XLSX。
4. 原生 SQL Server fallback 行为。
5. SQL Server/JDBC 排序后导出顺序保持一致。
6. JOIN 查询导出不启用 keyset，但结果正确。
7. 中途取消后进度、文件清理和 session 清理正确。

## 16. 验收标准

1. CSV/XLSX 查询结果导出期间，前端不再持有完整导出 rows。
2. 默认最多导出 `100000` 行。
3. 关闭上限后，CSV 可导出超过 `100000` 行。
4. XLSX 超过格式上限时给出明确错误。
5. 当前结果排序后导出的顺序一致。
6. JOIN/聚合/复杂 SQL 不误启用 keyset 优化。
7. 取消后不留下可误用的半成品文件。
8. agent query session 和 client session 被释放。
9. JSON/Markdown/SQL 导出未发生行为回归。

## 17. 建议实施顺序

1. 设置项拆分：`exportRowLimit`、`exportRowLimitEnabled`、`queryExportKeysetOptimizationEnabled`。
2. Rust core：实现 `query_result_export`、CSV rows formatter、XLSX 限制和取消清理。
3. 分页策略：先实现 agent/session 和 LIMIT/OFFSET，再实现安全 keyset 优化。
4. Tauri 命令和 Web 路由。
5. 前端 API 封装。
6. `queryStore.buildQueryResultExportRequest`。
7. `useDataGridExport` 接入 CSV/XLSX 新路径。
8. 单测和手工验证。

## 18. 设计自检

1. 没有把表数据导出的 keyset 规则直接套到任意查询结果。
2. 没有把 `exportBatchSize` 继续作为总上限。
3. 没有让 `queryBaseSql` 覆盖用户实际导出 SQL。
4. 明确了 XLSX 行数上限。
5. 明确了取消和 session 清理。
6. 明确了旧格式导出的兼容边界。
