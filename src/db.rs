use crate::{
    config::Profile,
    results::{Cells, MAX_BYTES, MAX_ROW, ResultSet},
};
use futures_util::{TryStreamExt, pin_mut};
use std::time::{Duration, Instant};
use tokio_postgres::{Client, SimpleQueryMessage};

pub struct Session {
    pub client: Client,
    connection: tokio::task::JoinHandle<()>,
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
        .connect_timeout(Duration::from_secs(15));
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
        Ok(Session { client, connection })
    } else {
        config.ssl_mode(tokio_postgres::config::SslMode::Disable);
        let (client, connection) = config.connect(tokio_postgres::NoTls).await.map_err(error)?;
        let connection = tokio::spawn(async move {
            let _ = connection.await;
        });
        Ok(Session { client, connection })
    }
}

pub fn tls_connector(profile: &Profile) -> Result<native_tls::TlsConnector, String> {
    native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(profile.trust_server_certificate)
        .danger_accept_invalid_hostnames(profile.trust_server_certificate)
        .build()
        .map_err(|e| e.to_string())
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

pub async fn execute(client: &Client, sql: &str, zh: bool, mut progress: impl FnMut(usize)) -> Result<(Vec<ResultSet>, u64), QueryFailure> {
    let stream = client.simple_query_raw(sql).await.map_err(query_error)?;
    pin_mut!(stream);
    let mut sets: Vec<ResultSet> = Vec::new();
    let mut bytes = 0;
    let mut affected: u64 = 0;
    let mut received = 0;
    let mut has_rowset = false;
    let mut last_progress = Instant::now();
    while let Some(message) = stream.try_next().await.map_err(query_error)? {
        match message {
            SimpleQueryMessage::RowDescription(columns) => {
                has_rowset = true;
                progress(received);
                if sets.len() >= 32 || columns.len() > 256 {
                    return Err("LIMIT_COLUMNS".into());
                }
                sets.push(ResultSet::new(
                    columns.iter().map(|c| c.name().to_owned()).collect(),
                )?);
            }
            SimpleQueryMessage::Row(row) => {
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
                if bytes > MAX_BYTES {
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
                if !has_rowset {
                    if sets.len() >= 32 { return Err("LIMIT_COLUMNS".into()); }
                    let mut set = ResultSet::new(vec![crate::tr(zh, "状态", "Status"), crate::tr(zh, "影响行数", "Affected rows")])?;
                    bytes += set.push(vec![Some(crate::tr(zh, "执行成功", "Success")), Some(count.to_string())])?;
                    if bytes > MAX_BYTES { return Err("LIMIT_RESULT".into()); }
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
