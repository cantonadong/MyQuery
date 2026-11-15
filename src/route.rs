//! Cross-connection statement routing.
//!
//! A table may be prefixed with a saved connection name: `conn.table`, `conn.schema.table`,
//! `conn.table.column`. Each statement is sent to the connection it names, with the prefix
//! removed. One statement can reference only one connection (PostgreSQL cannot join across
//! servers); consecutive statements for the same connection run as one batch on one session.

#[derive(Debug, PartialEq)]
pub struct Batch {
    pub connection: String,
    pub sql: String,
}

#[derive(Debug, PartialEq)]
pub enum RouteError {
    /// 1-based statement number and a short excerpt.
    Missing(usize, String),
    Mixed(usize, Vec<String>),
}

#[derive(Clone, Copy, PartialEq)]
enum Kind { Word, Quoted, Dot, Semicolon, Comma, Other }

struct Token { kind: Kind, start: usize, end: usize }

fn word_start(byte: u8) -> bool { byte.is_ascii_alphabetic() || byte == b'_' || byte >= 128 }
fn word_byte(byte: u8) -> bool { byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$' || byte >= 128 }

fn lex(sql: &str) -> Vec<Token> {
    let bytes = sql.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let byte = bytes[i];
        if byte.is_ascii_whitespace() { i += 1; continue; }
        if bytes[i..].starts_with(b"--") {
            while i < bytes.len() && bytes[i] != b'\n' { i += 1; }
            continue;
        }
        if bytes[i..].starts_with(b"/*") {
            i += 2;
            let mut depth = 1;
            while i < bytes.len() && depth > 0 {
                if bytes[i..].starts_with(b"/*") { depth += 1; i += 2; }
                else if bytes[i..].starts_with(b"*/") { depth -= 1; i += 2; }
                else { i += 1; }
            }
            continue;
        }
        let kind = if byte == b'\'' || ((byte == b'e' || byte == b'E') && bytes.get(i + 1) == Some(&b'\'')) {
            let escapes = byte != b'\'';
            i += if escapes { 2 } else { 1 };
            while i < bytes.len() {
                if escapes && bytes[i] == b'\\' { i += 2; continue; }
                if bytes[i] == b'\'' {
                    i += 1;
                    if bytes.get(i) == Some(&b'\'') { i += 1; } else { break; }
                } else { i += 1; }
            }
            Kind::Other
        } else if byte == b'"' {
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'"' {
                    i += 1;
                    if bytes.get(i) == Some(&b'"') { i += 1; } else { break; }
                } else { i += 1; }
            }
            Kind::Quoted
        } else if byte == b'$' && bytes.get(i + 1).is_some_and(|next| *next == b'$' || word_start(*next) && *next < 128) {
            let tag_end = sql[i + 1..].find('$').map(|end| i + 2 + end);
            match tag_end.filter(|end| sql[i + 1..end - 1].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')) {
                Some(end) => {
                    let delimiter = &sql[i..end];
                    i = sql[end..].find(delimiter).map_or(sql.len(), |pos| end + pos + delimiter.len());
                }
                None => i += 1,
            }
            Kind::Other
        } else if word_start(byte) {
            while i < bytes.len() && word_byte(bytes[i]) { i += 1; }
            Kind::Word
        } else if byte.is_ascii_digit() {
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'.' || bytes[i] == b'_') { i += 1; }
            Kind::Other
        } else {
            i += 1;
            while i < bytes.len() && !sql.is_char_boundary(i) { i += 1; }
            match byte { b'.' => Kind::Dot, b';' => Kind::Semicolon, b',' => Kind::Comma, _ => Kind::Other }
        };
        tokens.push(Token { kind, start, end: i });
    }
    tokens
}

fn identifier(sql: &str, token: &Token) -> String {
    let text = &sql[token.start..token.end];
    if token.kind == Kind::Quoted {
        text.strip_prefix('"').unwrap_or(text).strip_suffix('"').unwrap_or(text).replace("\"\"", "\"")
    } else { text.into() }
}

/// Resolves an identifier to a saved connection name. Quoted identifiers match exactly;
/// unquoted ones match exactly first, then case-insensitively when that is unambiguous.
fn connection<'a>(name: &str, quoted: bool, connections: &'a [String]) -> Option<&'a String> {
    if let Some(exact) = connections.iter().find(|candidate| candidate.as_str() == name) { return Some(exact); }
    if quoted { return None; }
    let lower = name.to_lowercase();
    let mut matches = connections.iter().filter(|candidate| candidate.to_lowercase() == lower);
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}

const FROM_KEYWORDS: [&str; 6] = ["from", "join", "update", "into", "table", "only"];
const CLAUSE_KEYWORDS: [&str; 15] = ["where", "on", "using", "group", "order", "having", "limit", "union", "intersect", "except", "set", "values", "select", "returning", "window"];
const INHERIT_KEYWORDS: [&str; 12] = ["begin", "start", "commit", "end", "rollback", "abort", "savepoint", "release", "set", "reset", "show", "discard"];

struct Statement { text: String, connections: Vec<String>, inherit: bool, excerpt: String }

/// Splits `sql` into connection batches. `default` is the connection used by statements without a
/// prefix; `None` means every statement must name its connection.
pub fn plan(sql: &str, connections: &[String], default: Option<&str>) -> Result<Vec<Batch>, RouteError> {
    let tokens = lex(sql);
    let mut statements = Vec::new();
    let mut first = 0;
    let mut routed = false;
    for index in 0..=tokens.len() {
        if index < tokens.len() && tokens[index].kind != Kind::Semicolon { continue; }
        let end = tokens.get(index).map_or(sql.len(), |token| token.start);
        let part = &tokens[first..index];
        if !part.is_empty() {
            let mut deletions = Vec::new();
            let mut names: Vec<String> = Vec::new();
            let mut in_from = false;
            for (i, token) in part.iter().enumerate() {
                if token.kind == Kind::Word {
                    let key = sql[token.start..token.end].to_ascii_lowercase();
                    if FROM_KEYWORDS.contains(&key.as_str()) { in_from = true; }
                    else if CLAUSE_KEYWORDS.contains(&key.as_str()) { in_from = false; }
                }
                if !matches!(token.kind, Kind::Word | Kind::Quoted) || (i > 0 && part[i - 1].kind == Kind::Dot) { continue; }
                let mut dots = 0;
                let mut cursor = i;
                while part.get(cursor + 1).is_some_and(|dot| dot.kind == Kind::Dot && dot.start == part[cursor].end)
                    && part.get(cursor + 2).is_some_and(|next| next.start == part[cursor + 1].end && (matches!(next.kind, Kind::Word | Kind::Quoted) || &sql[next.start..next.end] == "*")) {
                    dots += 1;
                    cursor += 2;
                }
                if dots == 0 { continue; }
                let from_context = i > 0 && match part[i - 1].kind {
                    Kind::Word => FROM_KEYWORDS.contains(&sql[part[i - 1].start..part[i - 1].end].to_ascii_lowercase().as_str()),
                    Kind::Comma => in_from,
                    _ => false,
                };
                if dots < 2 && !from_context { continue; }
                if let Some(name) = connection(&identifier(sql, token), token.kind == Kind::Quoted, connections) {
                    deletions.push(token.start..part[i + 1].end);
                    if !names.contains(name) { names.push(name.clone()); }
                }
            }
            routed |= !names.is_empty();
            let statement_start = part[0].start;
            let mut text = String::new();
            let mut cursor = statement_start;
            for range in &deletions {
                text.push_str(&sql[cursor..range.start]);
                cursor = range.end;
            }
            text.push_str(&sql[cursor..end]);
            let leading = sql[part[0].start..part[0].end].to_ascii_lowercase();
            let excerpt: String = sql[statement_start..end].split_whitespace().collect::<Vec<_>>().join(" ").chars().take(60).collect();
            statements.push(Statement { text: text.trim_end().into(), connections: names, inherit: part[0].kind == Kind::Word && INHERIT_KEYWORDS.contains(&leading.as_str()), excerpt });
        }
        first = index + 1;
    }
    if !routed {
        if let Some(default) = default {
            return Ok(vec![Batch { connection: default.into(), sql: sql.into() }]);
        }
    }
    let mut targets: Vec<Option<String>> = Vec::with_capacity(statements.len());
    for (index, statement) in statements.iter().enumerate() {
        targets.push(match statement.connections.len() {
            0 if statement.inherit => None,
            0 => Some(default.ok_or_else(|| RouteError::Missing(index + 1, statement.excerpt.clone()))?.to_string()),
            1 => Some(statement.connections[0].clone()),
            _ => return Err(RouteError::Mixed(index + 1, statement.connections.clone())),
        });
    }
    // Transaction and session statements (BEGIN, COMMIT, SET…) follow the neighbouring statement.
    let mut previous: Option<String> = None;
    for index in 0..targets.len() {
        if targets[index].is_none() {
            targets[index] = previous.clone()
                .or_else(|| targets[index..].iter().flatten().next().cloned())
                .or_else(|| default.map(Into::into));
        }
        match &targets[index] {
            Some(target) => previous = Some(target.clone()),
            None => return Err(RouteError::Missing(index + 1, statements[index].excerpt.clone())),
        }
    }
    let mut batches: Vec<Batch> = Vec::new();
    for (statement, target) in statements.into_iter().zip(targets.into_iter().flatten()) {
        match batches.last_mut().filter(|batch| batch.connection == target) {
            Some(batch) => { batch.sql.push_str(";\n"); batch.sql.push_str(&statement.text); }
            None => batches.push(Batch { connection: target, sql: statement.text }),
        }
    }
    Ok(batches)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Vec<String> { vec!["prod".into(), "Data Warehouse".into()] }

    #[test]
    fn statements_route_by_connection_prefix() {
        let sql = "SELECT o.id, prod.orders.total FROM prod.orders o; SELECT * FROM \"Data Warehouse\".public.users WHERE note = 'prod.orders'";
        let batches = plan(sql, &names(), Some("dev")).unwrap();
        assert_eq!(batches, vec![
            Batch { connection: "prod".into(), sql: "SELECT o.id, orders.total FROM orders o".into() },
            Batch { connection: "Data Warehouse".into(), sql: "SELECT * FROM public.users WHERE note = 'prod.orders'".into() },
        ]);
    }

    #[test]
    fn unprefixed_sql_is_untouched_and_requires_a_connection_when_all_are_selected() {
        let sql = "SELECT prod.id FROM prod;";
        assert_eq!(plan(sql, &names(), Some("dev")).unwrap(), vec![Batch { connection: "dev".into(), sql: sql.into() }]);
        assert_eq!(plan("SELECT 1", &names(), None), Err(RouteError::Missing(1, "SELECT 1".into())));
        assert!(matches!(plan("SELECT * FROM prod.a JOIN \"Data Warehouse\".b ON true", &names(), None), Err(RouteError::Mixed(1, _))));
    }

    #[test]
    fn transaction_statements_follow_their_neighbours() {
        let batches = plan("BEGIN; UPDATE prod.t SET a = 1; COMMIT; SELECT 1", &names(), Some("dev")).unwrap();
        assert_eq!(batches, vec![
            Batch { connection: "prod".into(), sql: "BEGIN;\nUPDATE t SET a = 1;\nCOMMIT".into() },
            Batch { connection: "dev".into(), sql: "SELECT 1".into() },
        ]);
    }
}
