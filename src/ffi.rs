use crate::{config::Profile, db};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{ffi::{CStr, CString, c_char}, path::PathBuf, time::Duration};

#[derive(Deserialize)]
struct Request {
    profile: Profile,
    password: String,
    sql: String,
    #[serde(default)]
    parameters: Vec<Option<String>>,
    output: PathBuf,
    timeout_seconds: u64,
    max_rows: usize,
    max_bytes: u64,
    #[serde(default)]
    typed: bool,
}

async fn query(request: &Request) -> Result<Value, String> {
    if request.timeout_seconds == 0 || request.max_rows == 0 || request.max_bytes == 0 || request.sql.trim().is_empty() {
        return Err("Invalid query limits or SQL".into());
    }
    let mut statement = request.sql.clone();
    if !request.parameters.is_empty() {
        let parts: Vec<_> = statement.split("%s").collect();
        if parts.len() != request.parameters.len() + 1 { return Err("Invalid query parameters".into()); }
        let mut bound = parts[0].to_owned();
        for (value, tail) in request.parameters.iter().zip(&parts[1..]) {
            bound.push_str(&value.as_ref().map(|value| db::literal(value, request.profile.redshift)).unwrap_or_else(|| "NULL".into()));
            bound.push_str(tail);
        }
        statement = bound;
    }
    let timeout = Duration::from_secs(request.timeout_seconds);
    let session = tokio::time::timeout(timeout, db::connect(&request.profile, &request.password)).await.map_err(|_| "TIMEOUT")??;
    let work = async {
        session.client.batch_execute(&format!("BEGIN READ ONLY; SET statement_timeout = {}", request.timeout_seconds.saturating_mul(1000))).await.map_err(db::error)?;
        let description = if request.typed {
            Some(session.client.prepare(&statement).await.map_err(db::error)?)
        } else { None };
        let (sets, _) = db::execute_limited(&session.client, &statement, |_| {}, request.max_rows, request.max_bytes).await.map_err(|error| error.message)?;
        let mut sets = sets.into_iter().filter(|set| !set.status);
        let mut result = sets.next().ok_or("NO_RESULT")?;
        if sets.next().is_some() { return Err("MULTIPLE_RESULTS".into()); }
        let oids: Vec<_> = description.as_ref().map(|statement| statement.columns().iter().map(|column| column.type_().oid()).collect()).unwrap_or_default();
        if let Some(description) = description.as_ref() {
            if description.columns().iter().map(|column| column.name()).ne(result.headers.iter().map(String::as_str)) {
                return Err("Result columns do not match query metadata".into());
            }
        }
        result.write_json_rows(&request.output)?;
        Ok(json!({"headers": result.headers, "oids": oids, "row_count": result.count}))
    };
    match tokio::time::timeout(timeout, work).await {
        Ok(result) => result,
        Err(_) => {
            let cancel = session.client.cancel_token();
            if request.profile.tls {
                if let Ok(tls) = db::tls_connector(&request.profile) {
                    let _ = tokio::time::timeout(Duration::from_secs(5), cancel.cancel_query(postgres_native_tls::MakeTlsConnector::new(tls))).await;
                }
            } else {
                let _ = tokio::time::timeout(Duration::from_secs(5), cancel.cancel_query(tokio_postgres::NoTls)).await;
            }
            Err("TIMEOUT".into())
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn myquery_core_version() -> u32 { 1 }

#[unsafe(no_mangle)]
pub unsafe extern "C" fn myquery_core_execute(input: *const c_char) -> *mut c_char {
    let outcome = std::panic::catch_unwind(|| {
        if input.is_null() { return json!({"error": "Missing query request"}); }
        let request = match unsafe { CStr::from_ptr(input) }.to_str().ok().and_then(|value| serde_json::from_str::<Request>(value).ok()) {
            Some(request) => request,
            None => return json!({"error": "Invalid query request"}),
        };
        let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(runtime) => runtime,
            Err(error) => return json!({"error": error.to_string()}),
        };
        match runtime.block_on(query(&request)) {
            Ok(result) => result,
            Err(error) => json!({"error": if request.password.is_empty() { error } else { error.replace(&request.password, "***") }}),
        }
    }).unwrap_or_else(|_| json!({"error": "Shared query engine failed"}));
    CString::new(outcome.to_string()).expect("JSON has no NUL bytes").into_raw()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn myquery_core_free(output: *mut c_char) {
    if !output.is_null() { drop(unsafe { CString::from_raw(output) }); }
}
