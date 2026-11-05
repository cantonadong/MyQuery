use crate::{SqlCompletion, syntax};

fn word(c: char) -> bool { c.is_alphanumeric() || matches!(c, '_' | '$') }
fn keyword(text: &str) -> bool { syntax::tokens(text).first().is_some_and(|(_, color)| *color == "#245dc1") }
fn start(sql: &str, offset: usize) -> usize {
    if let Some((range, _)) = syntax::tokens(&sql[..offset]).last().filter(|(range, color)| *color == "#94651c" && range.end == offset) { return range.start; }
    sql[..offset].char_indices().rev().find(|(_, c)| !word(*c) && *c != '"').map_or(0, |(i, c)| i + c.len_utf8())
}

pub fn insert(sql: &str, offset: usize, value: &str) -> SqlCompletion {
    let offset = offset.min(sql.len());
    if !sql.is_char_boundary(offset) { return SqlCompletion { text: sql.into(), offset: offset as i32 }; }
    let start = start(sql, offset);
    let mut end = offset;
    while end < sql.len() && sql[end..].chars().next().is_some_and(word) { end += sql[end..].chars().next().unwrap().len_utf8(); }
    if sql[start..].starts_with('"') && sql[end..].starts_with('"') { end += 1; }
    SqlCompletion { text: format!("{}{}{}", &sql[..start], value, &sql[end..]).into(), offset: (start + value.len()) as i32 }
}

fn name(value: &str) -> String {
    if value.chars().enumerate().all(|(i, c)| c == '_' || c.is_ascii_lowercase() || (i > 0 && (c.is_ascii_digit() || c == '$'))) && !keyword(value) { value.into() }
    else { crate::db::identifier(value) }
}

pub fn suggest(sql: &str, offset: usize, tables: &[(String, Vec<String>)], schema: &str, missing: &mut Option<String>) -> Vec<String> {
    let offset = offset.min(sql.len());
    if !sql.is_char_boundary(offset) || sql.len() > 64 * 1024 { return Vec::new(); }
    let ranges = syntax::tokens(sql);
    if ranges.iter().any(|(range, color)| matches!(*color, "#238047" | "#738276") && range.start < offset && offset <= range.end) { return Vec::new(); }
    let mut tokens = Vec::new();
    let mut i = 0;
    let mut range_index = 0;
    while i < sql.len() {
        while range_index < ranges.len() && ranges[range_index].0.end <= i { range_index += 1; }
        if let Some((range, _)) = ranges.get(range_index).filter(|(_, color)| matches!(*color, "#238047" | "#738276")) {
            i = range.end;
            continue;
        }
        let begin = i;
        let c = sql[i..].chars().next().unwrap();
        i += c.len_utf8();
        if c.is_whitespace() { continue; }
        if c == '"' {
            while i < sql.len() {
                let next = sql[i..].chars().next().unwrap();
                i += next.len_utf8();
                if next == '"' {
                    if sql[i..].starts_with('"') { i += 1; } else { break; }
                }
            }
        } else if word(c) {
            while i < sql.len() && sql[i..].chars().next().is_some_and(word) { i += sql[i..].chars().next().unwrap().len_utf8(); }
        }
        tokens.push((sql[begin..i].to_string(), begin, i));
    }
    let left = tokens.iter().rposition(|(text, _, end)| text == ";" && *end <= offset).map_or(0, |index| index + 1);
    let right = tokens.iter().position(|(text, begin, _)| text == ";" && *begin >= offset).unwrap_or(tokens.len());
    let tokens = &tokens[left..right];
    let fragment_start = start(sql, offset);
    let prefix = sql[fragment_start..offset].trim_matches('"').replace("\"\"", "\"").to_lowercase();
    let qualifier = tokens.iter().position(|(text, _, end)| text == "." && *end == fragment_start)
        .and_then(|index| index.checked_sub(1)).map(|index| tokens[index].0.trim_matches('"').replace("\"\"", "\""));
    if prefix.is_empty() && qualifier.is_none() { return Vec::new(); }
    // shortcut: resolve physical tables in this statement; add scoped parsing for CTE output inference.
    let mut references = Vec::new();
    let mut in_from = false;
    let mut index = 0;
    while index < tokens.len() {
        let key = tokens[index].0.to_ascii_lowercase();
        if matches!(key.as_str(), "where" | "group" | "order" | "having" | "limit" | "union" | "on" | "set") { in_from = false; }
        if matches!(key.as_str(), "from" | "join" | "update" | "into") || (key == "," && in_from) {
            in_from = true;
            index += 1;
            if index >= tokens.len() || tokens[index].0 == "(" { continue; }
            let mut table = tokens[index].0.trim_matches('"').replace("\"\"", "\"");
            if index + 2 < tokens.len() && tokens[index + 1].0 == "." {
                index += 2;
                table = tokens[index].0.trim_matches('"').replace("\"\"", "\"");
            }
            let mut alias = table.clone();
            let mut next = index + 1;
            if next < tokens.len() && tokens[next].0.eq_ignore_ascii_case("as") { next += 1; }
            if let Some((candidate, _, _)) = tokens.get(next) {
                if (candidate.starts_with('"') || candidate.chars().next().is_some_and(word)) && !keyword(candidate) {
                    alias = candidate.trim_matches('"').replace("\"\"", "\"");
                }
            }
            references.push((table, alias));
        }
        index += 1;
    }
    let only_table = if let Some(qualifier) = &qualifier {
        references.iter().find(|(table, alias)| alias.eq_ignore_ascii_case(qualifier) || table.eq_ignore_ascii_case(qualifier)).map(|(table, _)| table)
    } else if references.len() == 1 { Some(&references[0].0) } else { None };
    let mut choices = Vec::new();
    let table_context = tokens.iter().rev().find(|(_, _, end)| *end <= fragment_start).is_some_and(|(text, _, _)| matches!(text.to_ascii_lowercase().as_str(), "from" | "join" | "update" | "into"));
    if let Some(table) = only_table.filter(|_| !table_context) {
        if let Some((name, columns)) = tables.iter().find(|(candidate, _)| candidate.eq_ignore_ascii_case(table)) {
            if columns.is_empty() { *missing = Some(name.clone()); }
            choices.extend(columns.iter().cloned());
        }
    }
    if (table_context || only_table.is_none()) && qualifier.as_ref().is_none_or(|qualifier| qualifier.eq_ignore_ascii_case(schema)) {
        choices.extend(tables.iter().map(|(table, _)| table.clone()));
    }
    choices.retain(|value| value.to_lowercase().starts_with(&prefix) && value.to_lowercase() != prefix);
    choices.sort();
    choices.dedup();
    choices.truncate(50);
    choices.iter().map(|value| name(value)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_resolves_aliases_and_keeps_fields_in_the_filtered_table() {
        let tables = vec![("orders".into(), vec!["order_id".into()]), ("people".into(), vec!["person_id".into()])];
        let mut missing = None;
        for (sql, offset) in [("SELECT o. FROM orders AS o JOIN people AS p ON o.order_id = p.person_id", 9), ("SELECT ord FROM orders", 10)] {
            assert_eq!(suggest(sql, offset, &tables, "public", &mut missing), vec!["order_id"]);
        }
        assert!(suggest("SELECT p. FROM people p", 9, &tables[..1], "public", &mut missing).is_empty());
        assert_eq!(insert("SELECT ord_id FROM orders", 10, "order_id").text.as_str(), "SELECT order_id FROM orders");
    }
}
