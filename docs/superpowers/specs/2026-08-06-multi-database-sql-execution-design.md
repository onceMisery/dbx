# SQL 窗口多库执行设计 Spec

日期：2026-08-06
状态：Draft
范围：桌面端 SQL 查询窗口

## 1. 背景

当前 SQL 窗口的执行按钮只针对当前连接、database 和 schema 执行一次 SQL。用户在多个同类型数据库之间做巡检、配置同步或批量变更时，需要重复切换目标并执行相同 SQL。

本功能在 SQL 执行工具栏的执行按钮组末尾增加“多库执行”按钮。用户可以选择多个同类型的 `连接 + database + schema` 目标，确认后按顺序执行同一份 SQL，并为每个目标保留独立的结果 Tab。

实现重点是复用当前 SQL 执行链路，而不是重新实现一套查询协议。当前执行链路已经由 `useSqlExecution` 负责 SQL 解析和安全检查，由 `queryStore.executeTabSql` 负责连接、会话、结果生命周期、分页、取消和错误结果处理。

## 2. 已确认的产品决策

### 2.1 入口

- 在 `EditorToolbar.vue` 左侧执行按钮组的最后增加带文字的“多库执行”按钮。
- 仅在 query 类型 Tab 中显示。
- SQL 为空、普通执行/Explain 正在运行、当前 Tab 正在取消时禁用。
- 不新增快捷键，不改变普通执行按钮、编辑器快捷键和右键菜单行为。

### 2.2 执行目标

一个执行目标由以下字段组成：

```ts
interface MultiDbExecutionTarget {
  connectionId: string;
  catalog?: string;
  database: string;
  schema?: string;
}
```

- 只展示与当前连接有效数据库类型一致的连接。
- “有效数据库类型”沿用现有 `effectiveDatabaseTypeForConnection` 判断。
- 同一个目标的唯一键为 `connectionId + catalog + database + schema`。
- 支持跨同类型连接、database、schema 多选。
- Doris/StarRocks 等需要 catalog 的数据库额外保留 `catalog`；其他数据库不展示 catalog 层级。
- 当前连接当前 database/schema 默认勾选；当前 schema 为空时默认勾选当前连接和 database。

### 2.3 SQL 来源

多库执行沿用普通执行按钮的 SQL 选择规则：

- 编辑器有选中内容时执行选中 SQL。
- 没有选中内容时遵循当前执行模式，执行当前语句或全部 SQL。
- 参数展开、SQL 变量展开、语法方言处理和现有 SQL 解析逻辑保持一致。
- 点击按钮时捕获本次执行的 SQL 快照；弹窗打开后不允许编辑 SQL，避免用户看到的预览与实际执行内容不一致。
- 多库执行不改写 SQL 中显式写出的 database/schema 名称。目标的 database/schema 只作为该次执行的上下文传给现有执行接口。

### 2.4 SQL 类型

多库执行支持当前 SQL 窗口执行链路已经支持的全部 SQL 类型，包括但不限于：

- 查询：`SELECT`、`SHOW`、`EXPLAIN` 等；
- 数据变更：`INSERT`、`UPDATE`、`DELETE`、`MERGE`、`REPLACE` 等；
- 结构变更：`CREATE`、`ALTER`、`DROP`、`TRUNCATE` 等；
- 多语句脚本和当前数据库驱动支持的其他语句。

“多库执行”不是查询专用功能。非查询 SQL 的结果 Tab 继续使用现有结果模型展示 affected rows、执行耗时、错误信息或驱动返回的结果。

### 2.5 执行顺序和结果

- 目标按照弹窗中的稳定排序串行执行。
- 一个目标失败后继续执行剩余目标。
- 每个目标创建一个独立 query Tab，Tab 中保存该目标的 SQL、连接、database、schema、catalog 和执行结果。
- 不合并不同目标的结果集，不添加来源列。
- 不建立跨目标事务，也不承诺跨目标原子性。

### 2.6 安全检查

所有目标都逐个复用当前普通执行的安全检查：

- 危险 SQL 确认；
- 生产库 SQL 保护；
- Redis/Elasticsearch 等数据库类型的专属危险操作检查；
- 当前连接和驱动已有的权限、连接和执行错误处理。

安全确认被拒绝或取消时，只跳过当前目标并继续后续目标。用户点击“取消整个批次”时停止后续目标；已完成目标不回滚，当前正在执行的目标请求取消并等待执行链路收敛。

为了避免生成多个不可见的手动事务，第一期要求源 SQL Tab 处于自动提交模式且没有活动事务。`autoCommit === false` 或存在活动事务时，“多库执行”按钮禁用，并提示先提交/回滚或恢复自动提交。

## 3. 用户界面设计

### 3.1 工具栏按钮

文件：`apps/desktop/src/components/layout/EditorToolbar.vue`

新增事件：

```ts
"multiExecute": [];
```

按钮建议使用数据库/层叠数据库图标和文字标签，保持现有工具栏的紧凑高度。按钮 tooltip 使用“多库执行”。按钮 disabled 条件至少包括：

- `!executableSql.trim()`；
- `activeTab.isExecuting`；
- `activeTab.isExplaining`；
- `activeTab.isCancelling`；
- 当前 Tab 为手动事务模式或存在活动事务；
- 当前连接不支持 SQL 执行。

### 3.2 目标选择弹窗

新增组件：

`apps/desktop/src/components/editor/MultiDbExecuteDialog.vue`

弹窗打开后显示：

1. SQL 只读预览，可折叠，展示当前最终执行 SQL。
2. 目标搜索框，匹配连接名、database 名和 schema 名。
3. 按连接分组的树形选择器。
4. 已选择目标数量。
5. “取消”和“确认执行”按钮。

树形层级为：

```text
连接
└── catalog（仅适用时）
    └── database
        └── schema（仅适用时）
```

交互规则：

- 连接、catalog、database 父节点支持全选/取消全选。
- schema 节点支持多选。
- 只有完整目标才计入已选数量。
- 连接或 database 加载失败时，错误显示在对应节点下并提供重试。
- schema 按需加载，展开 database 时才请求，避免一次打开弹窗加载所有连接的全部 schema。
- 当前目标默认勾选。
- 目标类型不匹配的连接不展示，不使用“展示后禁用”的方式混入列表。
- 连接列表为空或没有有效 database/schema 时，显示空状态并禁用确认。

### 3.3 执行进度和最终汇总

确认后弹窗切换为执行状态，保留目标列表并显示每项目标状态：

- 等待中；
- 执行中；
- 成功；
- 失败；
- 已跳过；
- 已取消；
- 未执行。

进度信息至少包括：

- `已完成 / 总目标数`；
- 当前执行目标；
- 成功数、失败数、跳过数、未执行数；
- 当前目标耗时；
- 错误摘要。

执行完成后保留汇总页面，允许关闭弹窗。关闭弹窗不关闭已创建的结果 Tab，也不取消已经完成的目标。

## 4. 技术设计

### 4.1 组件和职责

| 单元 | 职责 |
| --- | --- |
| `EditorToolbar.vue` | 展示按钮并发出 `multiExecute` 事件 |
| `App.vue` | 打开弹窗，传递当前 SQL 快照，协调全局执行和取消 |
| `MultiDbExecuteDialog.vue` | 加载目标、选择目标、显示执行进度和最终汇总 |
| `useMultiDbExecution.ts` | 管理批次状态、串行队列、取消和目标结果映射 |
| `useSqlExecution.ts` | 抽取可复用的 SQL 解析、安全预检和参数处理能力 |
| `queryStore.ts` | 创建目标 Tab，调用 `executeTabSql`，保存结果和执行状态 |
| `useDatabaseOptions.ts` / `useSchemaOptions.ts` | 复用现有 database/schema 加载、过滤和排序规则 |
| `lib/backend/api.ts` | 不新增批量 API；继续通过现有 `executeQuery` 路径执行 |

### 4.2 批次状态模型

批次状态只存在于前端运行时，不写入打开 Tab 持久化数据：

```ts
type MultiDbExecutionItemStatus =
  | "pending"
  | "running"
  | "success"
  | "failed"
  | "skipped"
  | "cancelled"
  | "not_executed";

interface MultiDbExecutionItem {
  id: string;
  target: MultiDbExecutionTarget;
  tabId?: string;
  status: MultiDbExecutionItemStatus;
  errorMessage?: string;
  startedAt?: number;
  completedAt?: number;
}

interface MultiDbExecutionBatch {
  id: string;
  sourceTabId: string;
  sql: string;
  items: MultiDbExecutionItem[];
  status: "running" | "completed" | "cancelled";
  cancelRequested: boolean;
}
```

### 4.3 执行流程

```text
工具栏点击“多库执行”
        ↓
捕获普通执行规则解析后的 SQL 快照
        ↓
打开目标选择弹窗并按需加载同类型目标
        ↓ 确认
创建目标 query Tab（forceNew，保存目标上下文）
        ↓
按目标顺序串行处理
        ├─ 安全预检 / 用户确认
        ├─ 跳过目标或调用 queryStore.executeTabSql
        ├─ 写入该目标 Tab 的结果/错误状态
        └─ 更新批次进度
        ↓
显示成功/失败/跳过/未执行汇总
```

执行器必须通过目标 Tab 调用：

```ts
await queryStore.executeTabSql(item.tabId, batch.sql);
```

这样每次执行都会由 `queryStore` 从目标 Tab 读取正确的 `connectionId`、`database`、`schema` 和 `catalog`，并继续使用现有的：

- `ensureConnected`；
- 驱动选择和数据库类型判断；
- session/result session 管理；
- 多语句拆分和 statement 状态；
- 分页和结果缓存；
- execution id 和取消；
- affected rows、结果集和错误结果标准化。

`createTab` 需要增加可选的 `activate?: boolean`，默认保持现有行为为 `true`。多库执行创建目标 Tab 时传 `activate: false`，避免用户的当前 SQL 窗口在批次执行过程中反复跳转。

### 4.4 SQL 预处理复用

将 `useSqlExecution.ts` 当前与 `activeTab`、`activeConnection` 强绑定的预处理逻辑拆为可传入目标上下文的纯/半纯 helper，至少覆盖：

- `resolveExecutableSql`；
- SQL 变量展开；
- 参数识别和参数值准备；
- 危险 SQL 分类；
- 生产 SQL 评估；
- Redis/Elasticsearch 专属危险操作判断。

普通执行继续调用这些 helper，确保拆分后行为不变。多库执行对每个目标传入自己的连接配置、database、schema 和数据库类型，不能读取当前活动 Tab 作为安全判断依据。

参数处理规则：

- 同一批次只解析和收集一次参数；
- 参数值由用户确认后固定，所有目标使用同一组参数值；
- 某目标因参数不兼容失败时，只记录该目标失败并继续后续目标。

### 4.5 结果 Tab

每个目标创建独立 query Tab，标题使用可识别且不重复的格式：

```text
多库 · <连接名> · <catalog?> · <database> · <schema?>
```

Tab 保存：

- 原始/最终执行 SQL；
- connection/database/schema/catalog；
- 执行时间和执行状态；
- QueryResult 或标准化错误结果；
- DML/DDL 的 affected rows 和驱动返回信息。

多库执行不把目标 SQL 拼接成多语句，也不把多个目标的结果写入同一个 Tab。执行期间保留源 SQL Tab 作为活动 Tab；用户可从 Tab 栏打开任意目标结果。

## 5. 安全、事务和错误处理

### 5.1 安全确认

目标处理顺序中，安全预检必须早于 `executeTabSql`：

1. 检查目标连接是否仍存在且可执行；
2. 使用目标 connection/database 评估生产环境；
3. 使用目标数据库类型评估危险 SQL 或专属危险命令；
4. 需要确认时显示目标名称、database/schema 和 SQL 摘要；
5. 允许执行、跳过当前目标或取消整个批次。

安全确认不得因为源 Tab 是非生产环境而放行生产目标。确认文案必须明确“本次操作会在指定目标上执行”。

### 5.2 事务边界

- 多库批次不是分布式事务。
- 每个目标使用自己的执行连接和事务边界。
- 已成功目标不会因为后续目标失败而自动回滚。
- 批次取消只取消当前目标并阻止后续目标，不回滚已完成目标。
- 第一阶段禁止从手动事务模式启动多库执行，避免生成多个无法统一提交/回滚的事务。

### 5.3 失败继续

| 场景 | 当前目标 | 后续目标 |
| --- | --- | --- |
| 连接失败 | `failed`，保留错误 Tab | 继续 |
| SQL 执行抛错 | `failed`，保留错误结果 | 继续 |
| 返回 `execution_error` | `failed` | 继续 |
| 安全确认拒绝 | `skipped` | 继续 |
| 用户取消当前安全确认 | `skipped` | 继续 |
| 用户取消整个批次 | `cancelled` 或 `not_executed` | 停止 |
| 创建结果 Tab 失败 | `failed` | 继续 |

取消批次时，执行器需要等待当前 `cancelTabExecution` 完成或超时收敛，再将剩余 pending 项标记为 `not_executed`，避免界面显示批次已经结束但后端仍在执行。

## 6. 国际化和可访问性

新增 i18n 文案至少包括：

- `toolbar.multiDbExecute`；
- `toolbar.multiDbExecuteDisabledReason`；
- `multiDbExecute.title`；
- `multiDbExecute.sqlPreview`；
- `multiDbExecute.searchTargets`；
- `multiDbExecute.selectedCount`；
- `multiDbExecute.confirm`；
- `multiDbExecute.progress`；
- `multiDbExecute.success`；
- `multiDbExecute.failed`；
- `multiDbExecute.skipped`；
- `multiDbExecute.notExecuted`；
- `multiDbExecute.cancelBatch`；
- `multiDbExecute.noTargets`；
- `multiDbExecute.loadFailed`。

弹窗需满足现有 UI 组件的键盘和无障碍约定：

- 搜索框可直接获得焦点；
- 树节点支持键盘展开/收起和勾选；
- 确认、取消、取消批次按钮有明确 `aria-label`；
- 进度变化通过可读文本反映，不依赖颜色；
- 失败、跳过和未执行状态同时展示文字和图标。

## 7. 测试设计

### 7.1 弹窗组件测试

文件建议：

`apps/desktop/src/components/editor/__tests__/MultiDbExecuteDialog.spec.ts`

覆盖：

- 只展示同有效数据库类型连接；
- 当前目标默认勾选；
- catalog/database/schema 层级按能力显示；
- 连接、database、schema 父级全选和取消全选；
- 搜索过滤连接、database、schema；
- 目标唯一键去重；
- 未选择目标时确认按钮禁用；
- schema 按需加载、加载失败和重试；
- SQL 预览为只读；
- 确认事件序列化为 `MultiDbExecutionTarget[]`；
- 进度状态和最终汇总正确显示；
- 键盘操作和 aria 属性存在。

### 7.2 执行编排器测试

文件建议：

`apps/desktop/src/composables/__tests__/useMultiDbExecution.spec.ts`

覆盖：

- 按选择顺序串行调用 `executeTabSql`；
- 第一个目标失败后仍执行第二个目标；
- 每个目标创建独立 Tab，且目标上下文正确；
- 所有 SQL 类型都进入同一执行路径，不以 `SELECT` 作为过滤条件；
- 每个目标使用自己的生产环境和数据库类型进行安全判断；
- 安全确认拒绝只标记当前目标 `skipped`；
- 取消整个批次会取消当前目标并把剩余目标标记为 `not_executed`；
- 已完成目标在批次取消后保持结果；
- 目标 Tab 创建失败后继续后续目标；
- 批次完成汇总数量正确；
- SQL 快照和参数值在整个批次中保持稳定。

### 7.3 普通执行回归测试

- 普通执行按钮仍使用原有 `tryExecute` 流程；
- 普通执行的 SQL 选择规则不变；
- 危险 SQL、生产库保护和 Redis 专属安全逻辑不变；
- `createTab` 新增 `activate` 选项的默认行为不变；
- 单目标执行的 query result、分页、取消、事务和错误处理不变。

### 7.4 验证范围

由于本方案不新增 Rust/HTTP 批量执行接口，第一阶段不需要新增后端批量协议测试；需要继续执行现有前端类型检查、相关 Vitest 测试和普通 SQL 执行回归测试。

## 8. 预期文件变更

| 文件 | 变更 |
| --- | --- |
| `apps/desktop/src/components/layout/EditorToolbar.vue` | 增加“多库执行”按钮和事件 |
| `apps/desktop/src/components/editor/MultiDbExecuteDialog.vue` | 新增目标选择、进度和汇总弹窗 |
| `apps/desktop/src/composables/useMultiDbExecution.ts` | 新增串行执行编排器 |
| `apps/desktop/src/composables/useSqlExecution.ts` | 抽取可传入目标上下文的 SQL 预处理和安全预检 helper |
| `apps/desktop/src/stores/queryStore.ts` | 为 `createTab` 增加可选 `activate`，复用 `executeTabSql` |
| `apps/desktop/src/App.vue` | 打开弹窗、传递 SQL 快照、挂载批次执行和取消事件 |
| `apps/desktop/src/i18n/locales/*.ts` | 增加多库执行文案 |
| `apps/desktop/src/components/editor/__tests__/MultiDbExecuteDialog.spec.ts` | 弹窗测试 |
| `apps/desktop/src/composables/__tests__/useMultiDbExecution.spec.ts` | 编排器测试 |

不包含：Rust 查询执行逻辑、HTTP/Tauri API 协议、跨目标结果合并、后台任务调度和分布式事务。

## 9. 验收标准

1. SQL 工具栏执行按钮组末尾出现“多库执行”按钮。
2. 弹窗只展示同有效数据库类型的连接，并支持连接/database/schema 多选。
3. 当前连接当前 database/schema 默认选中。
4. SELECT、DML、DDL 和当前执行链路支持的其他 SQL 均可进入多库执行。
5. 多个目标按选择顺序串行执行，单个失败不阻塞后续目标。
6. 每个目标拥有独立结果 Tab，能显示查询结果、affected rows 或错误。
7. 每个目标独立执行现有安全检查，安全拒绝只跳过当前目标。
8. 用户取消批次后，当前执行可取消，后续目标不再执行，已完成结果保留。
9. 执行完成后显示成功、失败、跳过和未执行汇总。
10. 普通 SQL 执行行为、快捷键、结果生命周期和取消行为不回归。
