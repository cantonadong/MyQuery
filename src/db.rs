use crate::{
    config::Profile,
    results::{Cells, MAX_BYTES, MAX_ROW, ResultSet},
};
use futures_util::{TryStreamExt, pin_mut};
use std::{sync::Arc, time::{Duration, Instant}};
use tokio_postgres::{Client, SimpleQueryMessage};

pub struct Session {
    pub client: Arc<Client>,
    connection: tokio::task::JoinHandle<()>,
    heartbeat: tokio::task::JoinHandle<()>,
}

impl Session {
    fn new(client: Client, connection: tokio::task::JoinHandle<()>) -> Self {
        let client = Arc::new(client);
        let heartbeat_client = client.clone();
        let heartbeat = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                if heartbeat_client.check_connection().await.is_err() { break; }
            }
        });
        Self { client, connection, heartbeat }
    }
}

pub struct QueryFailure {
    pub message: String,
    pub recoverable: bool,
    pub state: Option<tokio_postgres::error::SqlState>,
}

impl From<String> for QueryFailure {
    fn from(message: String) -> Self {
        Self {
            message,
            recoverable: false,
            state: None,
        }
    }
}
impl From<&str> for QueryFailure {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}
pub fn query_error(source: tokio_postgres::Error) -> QueryFailure {
    let recoverable = source.as_db_error().is_some();
    let state = source.as_db_error().map(|error| error.code().clone());
    QueryFailure {
        message: error(source),
        recoverable,
        state,
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.heartbeat.abort();
        self.connection.abort();
    }
}

pub fn error(error: tokio_postgres::Error) -> String {
    if let Some(db) = error.as_db_error() {
        let mut message = format!("{} [{}]: {}", db.severity(), db.code().code(), db.message());
        if let Some(detail) = db.detail() {
            message.push_str(&format!("\n{detail}"));
        }
        if let Some(hint) = db.hint() {
            message.push_str(&format!("\n{hint}"));
        }
        if let Some(position) = db.position() {
            message.push_str(&format!("\n{position:?}"));
        }
        message
    } else {
        use std::error::Error;
        let mut message = error.to_string();
        let mut source = error.source();
        while let Some(cause) = source {
            message.push_str(&format!("\n{cause}"));
            source = cause.source();
        }
        message
    }
}

pub async fn connect(profile: &Profile, password: &str) -> Result<Session, String> {
    let mut config = tokio_postgres::Config::new();
    config
        .host(&profile.host)
        .port(profile.port)
        .dbname(&profile.database)
        .user(&profile.user)
        .password(password)
        .application_name("SqlDesk")
        .connect_timeout(Duration::from_secs(15))
        .keepalives(true)
        .keepalives_idle(Duration::from_secs(30))
        .keepalives_interval(Duration::from_secs(10));
    if profile.tls {
        config.ssl_mode(tokio_postgres::config::SslMode::Require);
        let tls = tls_connector(profile)?;
        let (client, connection) = config
            .connect(postgres_native_tls::MakeTlsConnector::new(tls))
            .await
            .map_err(error)?;
        let connection = tokio::spawn(async move {
            let _ = connection.await;
        });
        Ok(Session::new(client, connection))
    } else {
        config.ssl_mode(tokio_postgres::config::SslMode::Disable);
        let (client, connection) = config.connect(tokio_postgres::NoTls).await.map_err(error)?;
        let connection = tokio::spawn(async move {
            let _ = connection.await;
        });
        Ok(Session::new(client, connection))
    }
}

pub fn tls_connector(profile: &Profile) -> Result<native_tls::TlsConnector, String> {
    native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(profile.trust_server_certificate)
        .danger_accept_invalid_hostnames(profile.trust_server_certificate)
        .build()
        .map_err(|e| e.to_string())
}

pub fn columns_sql(schema: &str, table: Option<&str>, redshift: bool) -> String {
    if redshift {
        format!(
            "SELECT {}column_name, CASE WHEN LOWER(data_type) IN ('character varying', 'varchar', 'nvarchar', 'character', 'char', 'nchar', 'bpchar') AND character_maximum_length IS NOT NULL THEN CASE WHEN LOWER(data_type) IN ('character varying', 'varchar', 'nvarchar') THEN 'varchar' ELSE 'char' END || '(' || CAST(character_maximum_length AS VARCHAR(20)) || ')' WHEN LOWER(data_type) IN ('numeric', 'decimal') AND numeric_precision IS NOT NULL AND numeric_scale IS NOT NULL THEN data_type || '(' || CAST(numeric_precision AS VARCHAR(20)) || ',' || CAST(numeric_scale AS VARCHAR(20)) || ')' ELSE data_type END, is_nullable, column_default, CASE LOWER(data_type) WHEN 'smallint' THEN 2 WHEN 'int2' THEN 2 WHEN 'integer' THEN 4 WHEN 'int' THEN 4 WHEN 'int4' THEN 4 WHEN 'bigint' THEN 8 WHEN 'int8' THEN 8 WHEN 'real' THEN 4 WHEN 'float4' THEN 4 WHEN 'double precision' THEN 8 WHEN 'float8' THEN 8 WHEN 'float' THEN 8 WHEN 'boolean' THEN 1 WHEN 'bool' THEN 1 WHEN 'date' THEN 4 WHEN 'time' THEN 8 WHEN 'time without time zone' THEN 8 WHEN 'timetz' THEN 8 WHEN 'time with time zone' THEN 8 ELSE NULL END FROM svv_all_columns WHERE database_name = current_database() AND schema_name = {}{} ORDER BY {}ordinal_position",
            if table.is_none() { "table_name, " } else { "" },
            literal(schema, true),
            table.map(|table| format!(" AND table_name = {}", literal(table, true))).unwrap_or_default(),
            if table.is_none() { "table_name, " } else { "" },
        )
    } else {
        format!(
            "SELECT {}a.attname, pg_catalog.format_type(a.atttypid,a.atttypmod), CASE WHEN a.attnotnull THEN 'NO' ELSE 'YES' END, pg_catalog.pg_get_expr(d.adbin,d.adrelid), t.typlen FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_type t ON t.oid=a.atttypid JOIN pg_catalog.pg_class c ON c.oid=a.attrelid JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace LEFT JOIN pg_catalog.pg_attrdef d ON d.adrelid=a.attrelid AND d.adnum=a.attnum WHERE n.nspname={}{} AND a.attnum>0 AND NOT a.attisdropped ORDER BY {}a.attnum",
            if table.is_none() { "c.relname, " } else { "" },
            literal(schema, false),
            table.map(|table| format!(" AND c.relname={}", literal(table, false))).unwrap_or_else(|| " AND c.relkind IN ('r','p','f','v','m')".into()),
            if table.is_none() { "c.relname, " } else { "" },
        )
    }
}

pub async fn definition(client: &Client, schema: &str, name: &str, kind: i32, redshift: bool) -> Result<String, QueryFailure> {
    let qualified = format!("{}.{}", identifier(schema), identifier(name));
    if redshift {
        let command = if kind == 0 { "SHOW TABLE" } else { "SHOW VIEW" };
        let rows = metadata(client, &format!("{command} {qualified}")).await?;
        let text = rows.into_iter().flat_map(|row| row.into_iter().flatten()).collect::<Vec<_>>().join("\n");
        return if text.trim().is_empty() { Err("Object definition is unavailable".into()) } else { Ok(text) };
    }
    let relation = format!("c.relnamespace = n.oid AND n.nspname = {} AND c.relname = {}", literal(schema, false), literal(name, false));
    let rows = metadata(client, &format!("SELECT c.oid::text, c.relkind::text, CASE WHEN c.relkind IN ('v','m') THEN pg_catalog.pg_get_viewdef(c.oid, true) END FROM pg_catalog.pg_class c, pg_catalog.pg_namespace n WHERE {relation}")).await?;
    let row = rows.first().ok_or("Object definition is unavailable")?;
    let oid = row[0].as_deref().ok_or("Object definition is unavailable")?.parse::<u32>().map_err(|error| error.to_string())?;
    match row[1].as_deref() {
        Some("v" | "m") => {
            let keyword = if row[1].as_deref() == Some("m") { "MATERIALIZED VIEW" } else { "VIEW" };
            let sql = row[2].as_deref().filter(|text| !text.trim().is_empty()).ok_or("Object definition is unavailable")?;
            Ok(format!("CREATE {keyword} {qualified} AS\n{};", sql.trim().trim_end_matches(';')))
        }
        Some("r" | "p") => {
            let rows = metadata(client, &format!(
                "SELECT pg_catalog.quote_ident(a.attname), pg_catalog.format_type(a.atttypid,a.atttypmod), pg_catalog.pg_get_expr(d.adbin,d.adrelid), a.attnotnull::text, COALESCE(to_jsonb(a)->>'attidentity',''), COALESCE(to_jsonb(a)->>'attgenerated','') FROM pg_catalog.pg_attribute a LEFT JOIN pg_catalog.pg_attrdef d ON d.adrelid=a.attrelid AND d.adnum=a.attnum WHERE a.attrelid={oid} AND a.attnum>0 AND NOT a.attisdropped ORDER BY a.attnum"
            )).await?;
            let mut columns = Vec::new();
            for row in rows {
                let mut column = format!("    {} {}", row[0].as_deref().unwrap_or_default(), row[1].as_deref().unwrap_or_default());
                match row[4].as_deref() {
                    Some("a") => column.push_str(" GENERATED ALWAYS AS IDENTITY"),
                    Some("d") => column.push_str(" GENERATED BY DEFAULT AS IDENTITY"),
                    _ => if let Some(default) = &row[2] {
                        if matches!(row[5].as_deref(), Some("s" | "v")) {
                            column.push_str(&format!(" GENERATED ALWAYS AS ({default}) {}", if row[5].as_deref() == Some("v") { "VIRTUAL" } else { "STORED" }));
                        } else { column.push_str(&format!(" DEFAULT {default}")); }
                    },
                }
                if row[3].as_deref() == Some("true") { column.push_str(" NOT NULL"); }
                columns.push(column);
            }
            let constraints = metadata(client, &format!("SELECT pg_catalog.quote_ident(conname), pg_catalog.pg_get_constraintdef(oid, true) FROM pg_catalog.pg_constraint WHERE conrelid={oid} AND contype IN ('p','u','f','c','x') ORDER BY conname")).await?;
            for row in constraints {
                columns.push(format!("    CONSTRAINT {} {}", row[0].as_deref().unwrap_or_default(), row[1].as_deref().unwrap_or_default()));
            }
            let partition = if row[1].as_deref() == Some("p") {
                let rows = metadata(client, &format!("SELECT pg_catalog.pg_get_partkeydef({oid})")).await?;
                format!(" PARTITION BY {}", rows.first().and_then(|row| row[0].as_deref()).unwrap_or_default())
            } else { String::new() };
            let mut text = format!("CREATE TABLE {qualified} (\n{}\n){partition};", columns.join(",\n"));
            let indexes = metadata(client, &format!("SELECT pg_catalog.pg_get_indexdef(i.indexrelid) FROM pg_catalog.pg_index i WHERE i.indrelid={oid} AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_constraint c WHERE c.conindid=i.indexrelid) ORDER BY i.indexrelid")).await?;
            for row in indexes {
                if let Some(index) = &row[0] { text.push_str(&format!("\n\n{index};")); }
            }
            Ok(text)
        }
        _ => Err("Definition is unavailable for this object type".into()),
    }
}

pub async fn metadata(client: &Client, sql: &str) -> Result<Vec<Cells>, QueryFailure> {
    let stream = client.simple_query_raw(sql).await.map_err(query_error)?;
    pin_mut!(stream);
    let mut rows = Vec::new();
    let mut bytes = 0;
    while let Some(message) = stream.try_next().await.map_err(query_error)? {
        if let SimpleQueryMessage::Row(row) = message {
            if (0..row.len())
                .map(|i| row.get(i).map_or(0, str::len))
                .sum::<usize>()
                > MAX_ROW
            {
                return Err("LIMIT_METADATA".into());
            }
            let cells: Cells = (0..row.len())
                .map(|i| row.get(i).map(str::to_owned))
                .collect();
            bytes += cells.iter().flatten().map(String::len).sum::<usize>();
            if rows.len() >= 20_000 || bytes > 8 * 1024 * 1024 {
                return Err("LIMIT_METADATA".into());
            }
            rows.push(cells);
        }
    }
    Ok(rows)
}

pub async fn execute(client: &Client, sql: &str, progress: impl FnMut(usize)) -> Result<(Vec<ResultSet>, u64), QueryFailure> {
    execute_limited(client, sql, progress, usize::MAX, MAX_BYTES).await
}

pub async fn execute_limited(client: &Client, sql: &str, mut progress: impl FnMut(usize), max_rows: usize, max_bytes: u64) -> Result<(Vec<ResultSet>, u64), QueryFailure> {
    let stream = client.simple_query_raw(sql).await.map_err(query_error)?;
    pin_mut!(stream);
    let mut sets: Vec<ResultSet> = Vec::new();
    let mut bytes = 0;
    let mut affected: u64 = 0;
    let mut statement = 0;
    let mut received = 0;
    let mut has_rowset = false;
    let mut last_progress = Instant::now();
    while let Some(message) = stream.try_next().await.map_err(query_error)? {
        match message {
            SimpleQueryMessage::RowDescription(columns) => {
                has_rowset = true;
                progress(received);
                if columns.len() > 256 {
                    return Err("LIMIT_COLUMNS".into());
                }
                sets.push(ResultSet::new(
                    columns.iter().map(|c| c.name().to_owned()).collect(),
                )?);
            }
            SimpleQueryMessage::Row(row) => {
                if received >= max_rows { return Err("LIMIT_ROWS".into()); }
                if (0..row.len())
                    .map(|i| row.get(i).map_or(0, str::len))
                    .sum::<usize>()
                    > MAX_ROW
                {
                    return Err("LIMIT_ROW".into());
                }
                let cells = (0..row.len())
                    .map(|i| row.get(i).map(str::to_owned))
                    .collect();
                bytes += sets
                    .last_mut()
                    .ok_or("Missing result header")?
                    .push(cells)?;
                if bytes > max_bytes {
                    return Err("LIMIT_RESULT".into());
                }
                received += 1;
                if last_progress.elapsed() >= Duration::from_millis(100) {
                    progress(received);
                    last_progress = Instant::now();
                }
            }
            SimpleQueryMessage::CommandComplete(count) => {
                affected = affected.saturating_add(count);
                let mut set = if has_rowset {
                    sets.pop().ok_or("Missing result header")?
                } else {
                    let mut set = ResultSet::new(vec!["Status".into(), "Affected rows".into()])?;
                    set.status = true;
                    bytes += set.push(vec![Some("Success".into()), Some(count.to_string())])?;
                    if bytes > max_bytes { return Err("LIMIT_RESULT".into()); }
                    set
                };
                set.finish()?;
                set.statements.push(statement);
                statement += 1;
                let mut duplicate = false;
                for previous in &mut sets {
                    if previous.status == set.status && previous.same_content(&mut set)? {
                        previous.repeats += 1;
                        previous.statements.extend_from_slice(&set.statements);
                        duplicate = true;
                        break;
                    }
                }
                if !duplicate {
                    if sets.len() >= 32 { return Err("LIMIT_COLUMNS".into()); }
                    sets.push(set);
                }
                has_rowset = false;
            }
            _ => {}
        }
        tokio::task::yield_now().await;
    }
    for set in &mut sets {
        set.finish()?;
    }
    progress(received);
    Ok((sets, affected))
}

pub fn literal(value: &str, redshift: bool) -> String {
    let prefix = if redshift { "" } else { "E" };
    format!(
        "{prefix}'{}'",
        value.replace('\\', "\\\\").replace('\'', "''")
    )
}
pub fn identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::literal;

    #[test]
    fn metadata_literals_match_the_database_dialect() {
        assert_eq!(literal("public", true), "'public'");
        assert_eq!(literal("public", false), "E'public'");
        assert_eq!(literal("a\\'b", true), "'a\\\\''b'");
        assert_eq!(literal("a\\'b", false), "E'a\\\\''b'");
    }
}
