# CSV/Excel 批量导入真实环境性能基准

**测试日期：2026 年 7 月 29 日**

本基准测试对比父提交 `a96cf1194` 的实现与本 PR 优化后的导入路径。测试覆盖完整的文件解析和数据库写入流程，而不只是 SQL 生成过程。

## 测试环境

- 客户端：Windows 11 64 位，Intel Core i7-13620H（10 核、16 个逻辑处理器），31.7 GiB 内存。
- 工具链：Rust 1.96.0，使用 release profile，`dbx-core` 通过 `--no-default-features` 构建。
- SQL Server：SQL Server 2022 `16.0.4265.3`，运行于本地 Docker 容器。
- PostgreSQL：PostgreSQL `16.14`，远程可写测试实例；未隔离网络波动及服务器负载。
- 导入表结构：1 个 `BIGINT` 列和 11 个文本列，`batch_size = 500`。
- CSV 数据集：40,889,006 字节，200,000 行 × 12 列。
- Excel 数据集：4,559,480 字节（压缩后的 XLSX），100,000 行 × 12 列，共 120 万个单元格。

每个场景运行 3 次，基线版本和当前版本交替执行，结果表采用中位数。文件生成和数据库初始化不计入计时，文件解析和数据库写入计入计时。吞吐测试期间每 10 ms 采样一次进程 RSS。取消测试在首次收到写入进度后发出取消请求，取消延迟指从发出请求到导入 Future 返回所需的时间。

测量前已校验独立构建的可执行文件：

- 基线版本 SHA-256：`6E1C95139F3360EDBD0BBC5D739FEC5DBD628911C288BA339424271B088077D9`
- 优化版本 SHA-256：`51D79649B2475BF5A0B40549A6F04FC880344881149F9BF7E2157A157183BB5C`

## 测试结果

| 数据库 | 数据源 | 导入路径 | 文件大小（字节） | 行数 × 列数 | 耗时（ms） | 吞吐量（行/秒） | 峰值 RSS（MiB） | RSS 增量（MiB） | 取消延迟（ms） |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| SQL Server | CSV | 生成 INSERT（`a96cf1194`） | 40,889,006 | 200,000 × 12 | 38,568.1 | 5,185.6 | 20.04 | 5.76 | 0.833 |
| SQL Server | CSV | TDS Bulk + NVARCHAR 暂存表 | 40,889,006 | 200,000 × 12 | 8,142.1 | 24,563.7 | 20.21 | 5.49 | 0.760 |
| SQL Server | XLSX | 生成 INSERT（`a96cf1194`） | 4,559,480 | 100,000 × 12 | 19,667.9 | 5,084.4 | 19.68 | 4.41 | 0.624 |
| SQL Server | XLSX | TDS Bulk + NVARCHAR 暂存表 | 4,559,480 | 100,000 × 12 | 5,271.6 | 18,969.5 | 19.62 | 4.56 | 0.482 |
| PostgreSQL | CSV | 每个解析批次执行一次 COPY（`a96cf1194`） | 40,889,006 | 200,000 × 12 | 36,243.0 | 5,518.3 | 23.03 | 5.70 | 1.536 |
| PostgreSQL | CSV | 8 MiB / 5 万行 COPY 累加器 | 40,889,006 | 200,000 × 12 | 4,967.9 | 40,258.7 | 37.82 | 20.29 | 1.202 |
| PostgreSQL | XLSX | 每个解析批次执行一次 COPY（`a96cf1194`） | 4,559,480 | 100,000 × 12 | 22,838.6 | 4,378.6 | 22.59 | 4.05 | 0.912 |
| PostgreSQL | XLSX | 8 MiB / 5 万行 COPY 累加器 | 4,559,480 | 100,000 × 12 | 4,160.7 | 24,034.2 | 37.09 | 18.93 | 0.855 |

吞吐量中位数变化如下：

- SQL Server CSV：提升至 `4.74x`（`+373.7%`）。
- SQL Server Excel：提升至 `3.73x`（`+273.1%`）。
- PostgreSQL CSV：提升至 `7.30x`（`+629.5%`）。
- PostgreSQL Excel：提升至 `5.49x`（`+448.9%`）。

PostgreSQL COPY 累加器以有界的内存开销换取更少的网络往返次数。在这些数据集上，其峰值 RSS 中位数增加约 14～15 MiB；内存使用仍受 COPY 累加器、编码后批次、解析器和驱动缓冲区共同约束。远程 PostgreSQL 的 CSV 测试存在网络波动，优化版本的吞吐量范围为 17,600～42,300 行/秒；表中报告的 40,258.7 行/秒是中位数，而非最好成绩。

## 复现方式

数据库连接信息仅通过环境变量提供：

```powershell
$env:DBX_BENCH_HOST = '<host>'
$env:DBX_BENCH_PORT = '<port>'
$env:DBX_BENCH_USER = '<user>'
$env:DBX_BENCH_PASSWORD = '<password>'
$env:DBX_BENCH_DATABASE = '<database>'
$env:DBX_BENCH_SCHEMA = '<schema>'

cargo run -p dbx-core --no-default-features --release `
  --example table_import_live_bench -- `
  --database=postgres --format=csv --rows=200000 --columns=12 --batch-size=500
```

测试 SQL Server 时使用 `--database=sqlserver`；测试 Excel 场景时使用 `--format=xlsx --rows=100000`。基准程序会创建名称唯一的临时表，依次执行吞吐测试和取消测试，输出一条 JSON 结果，并在结束时删除测试表和临时文件。
