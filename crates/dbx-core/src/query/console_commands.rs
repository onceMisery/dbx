//! Executor-side dispatch for console client commands, parsed by
//! `dbx_sql_core::console_command` (Oracle SQL*Plus / YashanDB yasql, and
//! PostgreSQL psql backslash commands).
//!
//! Commands either resolve entirely on the client (`SET`, `PROMPT`, skips),
//! rewrite to SQL that runs through the normal statement path (`EXEC` → an
//! anonymous PL/SQL block, `SHOW USER` → a dictionary SELECT, `\d`/`\dt` →
//! pg_catalog SELECTs), or read metadata (`DESC`), which reuses the schema
//! column API so synonym and current-schema resolution behave exactly like
//! the object browser.

use dbx_sql::console_command::{parse_sqlplus_console_command, ConsoleCommand, ConsoleCommandFamily};

use crate::connection::AppState;
use crate::db;

/// What the executor should do for one console command.
#[derive(Debug)]
pub(crate) enum ConsoleExecution {
    /// SQL rewritten from the command; must run through the normal statement
    /// path so timeouts, risk classification and result handling apply.
    Sql(String),
    /// A finished client-side result (PROMPT, SET acknowledgement, skips).
    Result(db::QueryResult),
    /// A client-side failure (DESC of a missing object, RUN without a
    /// previous statement). Reported per statement but, like SQL*Plus SP2
    /// messages, it does not abort the batch.
    Error(String),
    /// A client-side result plus "stop the remaining statements" (EXIT).
    Stop(db::QueryResult),
}

/// Resolve one console command. `previous` is the last statement executed in
/// the current batch, needed by `RUN`; callers without batch context pass
/// `None` and `RUN` fails with a clear message.
pub(crate) async fn execute_console_command(
    state: &AppState,
    connection_id: &str,
    database: &str,
    schema: Option<&str>,
    command: ConsoleCommand,
    family: ConsoleCommandFamily,
    previous: Option<&str>,
) -> ConsoleExecution {
    match command {
        ConsoleCommand::Rerun => match previous {
            Some(previous) => match parse_for_family(previous, family) {
                // Re-running a console statement keeps its console semantics
                // (RUN after EXEC re-wraps the call, RUN after DESC re-reads
                // the metadata). `previous` never parses as Rerun because the
                // caller only records statements that actually executed.
                Some(previous_command) => {
                    Box::pin(execute_console_command(
                        state,
                        connection_id,
                        database,
                        schema,
                        previous_command,
                        family,
                        None,
                    ))
                    .await
                }
                None => ConsoleExecution::Sql(previous.to_string()),
            },
            None => ConsoleExecution::Error(
                "RUN has no previous statement in this execution; run it inside a script after another statement"
                    .to_string(),
            ),
        },
        ConsoleCommand::Describe { object } if family == ConsoleCommandFamily::Psql => {
            match psql_describe_sql(&object) {
                Some(sql) => ConsoleExecution::Sql(sql),
                None => ConsoleExecution::Error("DESCRIBE requires an object name".to_string()),
            }
        }
        ConsoleCommand::ListTables { pattern } => ConsoleExecution::Sql(psql_list_tables_sql(&pattern)),
        ConsoleCommand::Describe { object } => describe_object(state, connection_id, database, schema, &object).await,
        command => resolve_console_command_locally(command),
    }
}

/// Parse a statement with the family's parser (`RUN` re-dispatch).
fn parse_for_family(statement: &str, family: ConsoleCommandFamily) -> Option<ConsoleCommand> {
    match family {
        ConsoleCommandFamily::SqlPlus => parse_sqlplus_console_command(statement, true),
        // psql has no rerun command, so this arm only exists for symmetry.
        ConsoleCommandFamily::Psql => dbx_sql::console_command::parse_psql_console_command(statement),
    }
}

/// Commands that need no database round trip.
fn resolve_console_command_locally(command: ConsoleCommand) -> ConsoleExecution {
    match command {
        ConsoleCommand::Execute { call } => match exec_anonymous_block_sql(&call) {
            Some(sql) => ConsoleExecution::Sql(sql),
            None => ConsoleExecution::Error("EXECUTE requires a PL/SQL statement".to_string()),
        },
        ConsoleCommand::Show { item } => match item.trim() {
            "user" => ConsoleExecution::Sql("SELECT USER FROM DUAL".to_string()),
            "" => ConsoleExecution::Error("SHOW requires an item (supported: USER)".to_string()),
            other => ConsoleExecution::Result(console_message_result(&format!(
                "SHOW {other} is not supported in the query window; supported item: USER"
            ))),
        },
        ConsoleCommand::SetOption { option, value } => {
            let text = if value.trim().is_empty() {
                format!("SET {option} acknowledged (client console option)")
            } else {
                format!("SET {option} {value} acknowledged (client console option)")
            };
            ConsoleExecution::Result(console_message_result(&text))
        }
        ConsoleCommand::Prompt { text } => ConsoleExecution::Result(console_message_result(&text)),
        ConsoleCommand::Exit => ConsoleExecution::Stop(console_message_result(
            "EXIT acknowledged; remaining statements were skipped (the query window stays open)",
        )),
        // REMARK is SQL*Plus's comment command; it prints nothing.
        ConsoleCommand::Unsupported { verb } if verb == "REMARK" => {
            ConsoleExecution::Result(super::empty_query_result(0))
        }
        ConsoleCommand::Unsupported { verb } => ConsoleExecution::Result(console_message_result(&format!(
            "SQL*Plus command {verb} is not supported in the query window; statement skipped"
        ))),
        ConsoleCommand::Describe { .. } | ConsoleCommand::Rerun | ConsoleCommand::ListTables { .. } => {
            unreachable!("describe, list and rerun are resolved by execute_console_command")
        }
    }
}

/// `EXEC call` wraps the call in an anonymous PL/SQL block, exactly like
/// SQL*Plus does before sending it to the server. A trailing `--` comment
/// moves after `END;`: left in place it would comment out the injected `;`
/// and break the block. `None` when nothing executable remains.
fn exec_anonymous_block_sql(call: &str) -> Option<String> {
    let trimmed = call.trim().trim_end_matches(';').trim_end();
    let (body, comment) = split_trailing_line_comment(trimmed);
    if body.trim().is_empty() {
        return None;
    }
    Some(match comment {
        Some(comment) => format!("BEGIN\n{body};\nEND; {comment}"),
        None => format!("BEGIN\n{body};\nEND;"),
    })
}

/// Split `code -- comment` at a `--` that sits outside ordinary `'...'`
/// literals (Oracle q-quote literals are not tracked; a `--` inside one in a
/// single-line EXEC call is vanishingly rare).
fn split_trailing_line_comment(text: &str) -> (&str, Option<&str>) {
    let mut in_single_quote = false;
    for (index, ch) in text.char_indices() {
        match ch {
            '\'' => in_single_quote = !in_single_quote,
            '-' if !in_single_quote && text[index + 1..].starts_with('-') => {
                return (&text[..index].trim_end(), Some(text[index..].trim_end()));
            }
            _ => {}
        }
    }
    (text.trim_end(), None)
}

/// `DESC [OWNER.]TABLE` reads column metadata through the same API the object
/// browser uses, so owner qualification, synonyms and the current schema
/// resolve identically for the Go (Oracle) and JVM (YashanDB) agents.
async fn describe_object(
    state: &AppState,
    connection_id: &str,
    database: &str,
    schema: Option<&str>,
    object: &str,
) -> ConsoleExecution {
    let object = object.trim();
    if object.is_empty() {
        return ConsoleExecution::Error("DESCRIBE requires an object name".to_string());
    }
    let (owner, table) = split_owner_qualified_object(object);
    let schema_arg = owner.or(schema).unwrap_or("");
    match crate::schema::get_columns_core_for_session_in_context(
        state,
        connection_id,
        database,
        schema_arg,
        table,
        None,
        None,
    )
    .await
    {
        Ok(columns) if !columns.is_empty() => ConsoleExecution::Result(describe_result_from_columns(&columns, object)),
        Ok(_) => ConsoleExecution::Error(format!("DESCRIBE: no columns found for {object}")),
        Err(error) => ConsoleExecution::Error(format!("DESCRIBE {object} failed: {error}")),
    }
}

/// Split `OWNER.TABLE`; each part may be double-quoted. A dot inside a quoted
/// name cannot be distinguished from the qualifier, matching SQL*Plus.
fn split_owner_qualified_object(object: &str) -> (Option<&str>, &str) {
    fn unquote(part: &str) -> &str {
        part.trim().trim_matches('"')
    }
    match object.split_once('.') {
        Some((owner, table)) => {
            let (owner, table) = (unquote(owner), unquote(table));
            if owner.is_empty() || table.is_empty() {
                (None, unquote(object))
            } else {
                (Some(owner), table)
            }
        }
        None => (None, unquote(object)),
    }
}

/// Split `SCHEMA.TABLE` keeping each part's quotes, so
/// [`normalize_pg_identifier`] can still tell quoted identifiers (exact) from
/// unquoted ones (lowercase-folded).
fn split_pg_qualified_object(object: &str) -> (Option<&str>, &str) {
    match object.split_once('.') {
        Some((schema, table)) if !schema.trim().is_empty() && !table.trim().is_empty() => {
            (Some(schema.trim()), table.trim())
        }
        _ => (None, object.trim()),
    }
}

/// `\d [SCHEMA.]TABLE` rewrites to a pg_catalog query through the normal
/// statement path, so search-path resolution (`current_schemas`) and type
/// rendering (`format_type`) behave exactly like psql. Unquoted identifiers
/// fold to lowercase, as PostgreSQL does.
fn psql_describe_sql(object: &str) -> Option<String> {
    let object = object.trim();
    if object.is_empty() {
        return None;
    }
    let (owner, table) = split_pg_qualified_object(object);
    let table_literal = pg_string_literal(&normalize_pg_identifier(table));
    let schema_condition = match owner {
        Some(owner) => format!("n.nspname = {}", pg_string_literal(&normalize_pg_identifier(owner))),
        None => "n.nspname = ANY (current_schemas(true))".to_string(),
    };
    Some(format!(
        "SELECT a.attname AS \"Name\", \
CASE WHEN a.attnotnull THEN 'NOT NULL' ELSE '' END AS \"Null?\", \
format_type(a.atttypid, a.atttypmod) AS \"Type\" \
FROM pg_attribute a \
JOIN pg_class c ON c.oid = a.attrelid \
JOIN pg_namespace n ON n.oid = c.relnamespace \
WHERE c.relname = {table_literal} AND ({schema_condition}) AND a.attnum > 0 AND NOT a.attisdropped \
ORDER BY a.attnum"
    ))
}

/// `\dt [PATTERN]` rewrites to a pg_catalog query listing tables and views.
/// A psql pattern (`*`/`?` wildcards, optional `schema.` prefix) becomes
/// equality or LIKE; without a pattern the search path is listed, like psql.
fn psql_list_tables_sql(pattern: &str) -> String {
    let pattern = pattern.trim();
    let mut conditions = vec![
        "c.relkind IN ('r', 'p', 'v', 'm', 'f')".to_string(),
        "n.nspname NOT IN ('pg_catalog', 'information_schema')".to_string(),
        r"n.nspname NOT LIKE 'pg\_%' ESCAPE '\'".to_string(),
    ];
    let (schema_pattern, table_pattern) = match pattern.split_once('.') {
        Some((schema, table)) => (Some(schema.trim()), table.trim()),
        None => (None, pattern),
    };
    match schema_pattern {
        Some(schema_pattern) => {
            conditions.push(pg_pattern_condition("n.nspname", schema_pattern));
            if !table_pattern.is_empty() {
                conditions.push(pg_pattern_condition("c.relname", table_pattern));
            }
        }
        None => {
            // psql lists the search path unless the pattern names a schema.
            conditions.push("n.nspname = ANY (current_schemas(false))".to_string());
            if !table_pattern.is_empty() {
                conditions.push(pg_pattern_condition("c.relname", table_pattern));
            }
        }
    }
    format!(
        "SELECT n.nspname AS \"Schema\", c.relname AS \"Name\", \
CASE c.relkind WHEN 'r' THEN 'table' WHEN 'p' THEN 'partitioned table' WHEN 'v' THEN 'view' \
WHEN 'm' THEN 'materialized view' WHEN 'f' THEN 'foreign table' END AS \"Type\" \
FROM pg_class c \
JOIN pg_namespace n ON n.oid = c.relnamespace \
WHERE {} \
ORDER BY n.nspname, c.relname",
        conditions.join("\n  AND ")
    )
}

/// `column = 'value'` or `column LIKE 'va%' ESCAPE '\'` for one psql pattern.
/// A pattern without wildcards is exact equality (with identifier folding).
fn pg_pattern_condition(column: &str, pattern: &str) -> String {
    if !pattern.contains('*') && !pattern.contains('?') {
        return format!("{column} = {}", pg_string_literal(&normalize_pg_identifier(pattern)));
    }
    let mut like = String::new();
    for ch in pattern.chars() {
        match ch {
            '*' => like.push('%'),
            '?' => like.push('_'),
            '%' | '_' | '\\' => {
                like.push('\\');
                like.push(ch);
            }
            _ => like.push(ch),
        }
    }
    format!(r"{column} LIKE {} ESCAPE '\'", pg_string_literal(&like))
}

/// Unquoted PostgreSQL identifiers fold to lowercase; `"Quoted"` stay exact.
fn normalize_pg_identifier(identifier: &str) -> String {
    if identifier.starts_with('"') && identifier.ends_with('"') && identifier.len() >= 2 {
        identifier[1..identifier.len() - 1].to_string()
    } else {
        identifier.to_lowercase()
    }
}

/// Single-quoted SQL literal with `''` escaping.
fn pg_string_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Column metadata rendered in the SQL*Plus DESCRIBE layout: Name / Null? /
/// Type. Column order is the metadata order.
fn describe_result_from_columns(columns: &[db::ColumnInfo], object: &str) -> db::QueryResult {
    db::QueryResult {
        columns: vec!["Name".to_string(), "Null?".to_string(), "Type".to_string()],
        column_types: Vec::new(),
        column_sortables: vec![true, false, true],
        spatial_columns: vec![],
        spatial_values: vec![],
        rows: columns
            .iter()
            .map(|column| {
                vec![
                    serde_json::Value::String(column.name.clone()),
                    serde_json::Value::String(if column.is_nullable { String::new() } else { "NOT NULL".to_string() }),
                    serde_json::Value::String(column.data_type.clone()),
                ]
            })
            .collect(),
        affected_rows: 0,
        execution_time_ms: 0,
        server_execute_time_us: None,
        query_timings_ms: None,
        truncated: false,
        session_id: None,
        has_more: false,
        elasticsearch_raw_body: None,
        messages: Vec::new(),
    }
}

/// A one-column message grid, the shape SQL Server PRINT results already use,
/// so the frontend renders console output without a new result kind.
pub(crate) fn console_message_result(message: &str) -> db::QueryResult {
    db::QueryResult {
        columns: vec!["Message".to_string()],
        column_types: Vec::new(),
        column_sortables: vec![false],
        spatial_columns: vec![],
        spatial_values: vec![],
        rows: vec![vec![serde_json::Value::String(message.to_string())]],
        affected_rows: 0,
        execution_time_ms: 0,
        server_execute_time_us: None,
        query_timings_ms: None,
        truncated: false,
        session_id: None,
        has_more: false,
        elasticsearch_raw_body: None,
        messages: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_rewrites_to_anonymous_block() {
        assert_eq!(exec_anonymous_block_sql("p(1)"), Some("BEGIN\np(1);\nEND;".to_string()));
        assert_eq!(exec_anonymous_block_sql(":x := 5;"), Some("BEGIN\n:x := 5;\nEND;".to_string()));
        assert_eq!(
            exec_anonymous_block_sql(" dbms_stats.gather_table_stats('S','T') "),
            Some("BEGIN\ndbms_stats.gather_table_stats('S','T');\nEND;".to_string())
        );
        // A trailing line comment must not swallow the injected `;`.
        assert_eq!(exec_anonymous_block_sql("p(1) -- note"), Some("BEGIN\np(1);\nEND; -- note".to_string()));
        // `--` inside a string literal is not a comment.
        assert_eq!(exec_anonymous_block_sql("f('-')"), Some("BEGIN\nf('-');\nEND;".to_string()));
        // Nothing executable left after stripping the comment.
        assert_eq!(exec_anonymous_block_sql("-- note"), None);
        assert_eq!(exec_anonymous_block_sql(""), None);
    }

    #[test]
    fn local_commands_map_to_message_results() {
        match resolve_console_command_locally(ConsoleCommand::SetOption {
            option: "TIMING".to_string(),
            value: "ON".to_string(),
        }) {
            ConsoleExecution::Result(result) => {
                assert_eq!(result.columns, vec!["Message"]);
                assert_eq!(result.rows.len(), 1);
                let text = result.rows[0][0].as_str().unwrap();
                assert!(text.contains("SET TIMING ON acknowledged"), "{text}");
            }
            other => panic!("unexpected {other:?}"),
        }
        // `SET HEADING` without a value keeps a single-spaced message.
        match resolve_console_command_locally(ConsoleCommand::SetOption {
            option: "HEADING".to_string(),
            value: String::new(),
        }) {
            ConsoleExecution::Result(result) => {
                let text = result.rows[0][0].as_str().unwrap();
                assert_eq!(text, "SET HEADING acknowledged (client console option)");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(matches!(resolve_console_command_locally(ConsoleCommand::Exit), ConsoleExecution::Stop(_)));
        match resolve_console_command_locally(ConsoleCommand::Unsupported { verb: "SPOOL".to_string() }) {
            ConsoleExecution::Result(result) => assert_eq!(result.rows.len(), 1),
            other => panic!("unexpected {other:?}"),
        }
        // REMARK is a silent comment.
        match resolve_console_command_locally(ConsoleCommand::Unsupported { verb: "REMARK".to_string() }) {
            ConsoleExecution::Result(result) => assert!(result.rows.is_empty()),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn show_user_rewrites_and_others_report_supported_items() {
        assert!(matches!(
            resolve_console_command_locally(ConsoleCommand::Show { item: "user".to_string() }),
            ConsoleExecution::Sql(sql) if sql == "SELECT USER FROM DUAL"
        ));
        assert!(matches!(
            resolve_console_command_locally(ConsoleCommand::Show { item: "sga".to_string() }),
            ConsoleExecution::Result(_)
        ));
    }

    #[test]
    fn owner_qualified_objects_split_with_quotes() {
        assert_eq!(split_owner_qualified_object("SCOTT.EMP"), (Some("SCOTT"), "EMP"));
        assert_eq!(split_owner_qualified_object("emp"), (None, "emp"));
        assert_eq!(split_owner_qualified_object("\"My Table\""), (None, "My Table"));
        assert_eq!(split_owner_qualified_object("\"SCOTT\".\"EMP\""), (Some("SCOTT"), "EMP"));
        assert_eq!(split_owner_qualified_object(".EMP"), (None, ".EMP"));
        assert_eq!(split_owner_qualified_object("SCOTT."), (None, "SCOTT."));
    }

    #[test]
    fn describe_result_uses_sqlplus_layout() {
        let columns = vec![
            db::ColumnInfo {
                name: "EMPNO".to_string(),
                data_type: "NUMBER(4)".to_string(),
                is_nullable: false,
                ..db::ColumnInfo::default()
            },
            db::ColumnInfo {
                name: "ENAME".to_string(),
                data_type: "VARCHAR2(10)".to_string(),
                is_nullable: true,
                ..db::ColumnInfo::default()
            },
        ];
        let result = describe_result_from_columns(&columns, "emp");
        assert_eq!(result.columns, vec!["Name", "Null?", "Type"]);
        assert_eq!(result.rows.len(), 2);
        assert_eq!(result.rows[0][0], serde_json::json!("EMPNO"));
        assert_eq!(result.rows[0][1], serde_json::json!("NOT NULL"));
        assert_eq!(result.rows[0][2], serde_json::json!("NUMBER(4)"));
        assert_eq!(result.rows[1][1], serde_json::json!(""));
    }

    #[test]
    fn psql_describe_rewrites_to_pg_catalog_query() {
        let sql = psql_describe_sql("emp").unwrap();
        assert!(sql.contains("c.relname = 'emp'"));
        assert!(sql.contains("current_schemas(true)"));
        assert!(sql.contains("format_type(a.atttypid, a.atttypmod)"));
        let qualified = psql_describe_sql("public.EMP").unwrap();
        assert!(qualified.contains("n.nspname = 'public'"));
        assert!(qualified.contains("c.relname = 'emp'"), "unquoted names fold to lowercase");
        let quoted = psql_describe_sql("\"MyTable\"").unwrap();
        assert!(quoted.contains("c.relname = 'MyTable'"));
        let escaped = psql_describe_sql("it's").unwrap();
        assert!(escaped.contains("c.relname = 'it''s'"));
        assert!(psql_describe_sql("").is_none());
    }

    #[test]
    fn psql_list_tables_handles_patterns() {
        let plain = psql_list_tables_sql("");
        assert!(plain.contains("current_schemas(false)"));
        assert!(plain.contains("c.relkind IN ('r', 'p', 'v', 'm', 'f')"));
        let pattern = psql_list_tables_sql("t*");
        assert!(pattern.contains(r"c.relname LIKE 't%' ESCAPE '\'"));
        let escaped_wildcards = psql_list_tables_sql("100%*");
        assert!(escaped_wildcards.contains(r"c.relname LIKE '100\%%' ESCAPE '\'"));
        // Patterns without psql wildcards match exactly, keeping % literal.
        let literal_percent = psql_list_tables_sql("a_b%");
        assert!(literal_percent.contains("c.relname = 'a_b%'"));
        let exact = psql_list_tables_sql("EMP");
        assert!(exact.contains("c.relname = 'emp'"));
        let qualified = psql_list_tables_sql("public.t?x");
        assert!(qualified.contains("n.nspname = 'public'"));
        assert!(qualified.contains(r"c.relname LIKE 't_x' ESCAPE '\'"));
        assert!(!qualified.contains("current_schemas"));
    }
}
