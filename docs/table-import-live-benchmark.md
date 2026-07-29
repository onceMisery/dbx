# CSV/Excel bulk import live benchmark

This benchmark compares the parent implementation at `a96cf1194` with the optimized import paths in this PR.
It exercises the complete file parsing and database write pipeline rather than SQL generation alone.

## Environment

- Client: Windows 11 64-bit, Intel Core i7-13620H (10 cores / 16 logical processors), 31.7 GiB RAM.
- Toolchain: Rust 1.96.0, release profile, `dbx-core` built with `--no-default-features`.
- SQL Server: SQL Server 2022 `16.0.4265.3`, local Docker container.
- PostgreSQL: PostgreSQL `16.14`, remote writable test instance. Network and server load were not isolated.
- Import schema: one `BIGINT` column and eleven text columns, `batch_size = 500`.
- CSV dataset: 40,889,006 bytes, 200,000 rows x 12 columns.
- Excel dataset: 4,559,480 bytes (compressed XLSX), 100,000 rows x 12 columns (1.2 million cells).

Each scenario ran three times in alternating baseline/current order. The table reports medians. File generation and
database setup are outside the timed interval; parsing and database writes are included. Process RSS is sampled every
10 ms during the throughput import. Cancellation is requested after the first reported writing progress and measures
the time until the import future returns.

The independently built executables were checked before measurement:

- Baseline SHA-256: `6E1C95139F3360EDBD0BBC5D739FEC5DBD628911C288BA339424271B088077D9`
- Optimized SHA-256: `51D79649B2475BF5A0B40549A6F04FC880344881149F9BF7E2157A157183BB5C`

## Results

| Database | Source | Import path | File bytes | Rows x columns | Elapsed (ms) | Throughput (rows/s) | Peak RSS (MiB) | RSS delta (MiB) | Cancel latency (ms) |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| SQL Server | CSV | generated INSERT (`a96cf1194`) | 40,889,006 | 200,000 x 12 | 38,568.1 | 5,185.6 | 20.04 | 5.76 | 0.833 |
| SQL Server | CSV | TDS Bulk + NVARCHAR staging | 40,889,006 | 200,000 x 12 | 8,142.1 | 24,563.7 | 20.21 | 5.49 | 0.760 |
| SQL Server | XLSX | generated INSERT (`a96cf1194`) | 4,559,480 | 100,000 x 12 | 19,667.9 | 5,084.4 | 19.68 | 4.41 | 0.624 |
| SQL Server | XLSX | TDS Bulk + NVARCHAR staging | 4,559,480 | 100,000 x 12 | 5,271.6 | 18,969.5 | 19.62 | 4.56 | 0.482 |
| PostgreSQL | CSV | COPY per parser batch (`a96cf1194`) | 40,889,006 | 200,000 x 12 | 36,243.0 | 5,518.3 | 23.03 | 5.70 | 1.536 |
| PostgreSQL | CSV | 8 MiB / 50k-row COPY accumulator | 40,889,006 | 200,000 x 12 | 4,967.9 | 40,258.7 | 37.82 | 20.29 | 1.202 |
| PostgreSQL | XLSX | COPY per parser batch (`a96cf1194`) | 4,559,480 | 100,000 x 12 | 22,838.6 | 4,378.6 | 22.59 | 4.05 | 0.912 |
| PostgreSQL | XLSX | 8 MiB / 50k-row COPY accumulator | 4,559,480 | 100,000 x 12 | 4,160.7 | 24,034.2 | 37.09 | 18.93 | 0.855 |

Median throughput changed as follows:

- SQL Server CSV: `4.74x` (`+373.7%`).
- SQL Server Excel: `3.73x` (`+273.1%`).
- PostgreSQL CSV: `7.30x` (`+629.5%`).
- PostgreSQL Excel: `5.49x` (`+448.9%`).

The PostgreSQL accumulator intentionally trades bounded memory for fewer network round trips. Its median peak RSS
increased by about 14-15 MiB in these datasets while remaining bounded by the accumulator, encoded batch, parser, and
driver buffers. The remote PostgreSQL CSV runs showed network variance (optimized runs ranged from 17.6k to 42.3k
rows/s); the reported 40.3k rows/s is the median, not the best run.

## Reproduction

Connection values are supplied only through environment variables:

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

Use `--database=sqlserver` for SQL Server and `--format=xlsx --rows=100000` for the Excel scenario. The benchmark
creates a uniquely named table, performs throughput and cancellation runs, prints one JSON result, and removes its
table and temporary files.
