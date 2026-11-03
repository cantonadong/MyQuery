use std::ops::Range;

const KEYWORDS: &str = "select from where group by order having limit offset join left right full inner outer cross on as and or not null is in exists between like ilike distinct all union intersect except case when then else end with recursive over partition rows range unbounded preceding following current row asc desc nulls first last create alter drop table view materialized insert into values update set delete truncate begin commit rollback explain analyze count sum avg min max coalesce cast extract date timestamp interval true false grant revoke returning qualify window fetch top primary key references default";

pub fn tokens(sql: &str) -> Vec<(Range<usize>, &'static str)> {
    let bytes = sql.as_bytes();
    let mut result: Vec<(Range<usize>, &'static str)> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let color;
        if bytes[i..].starts_with(b"--") {
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            color = "#738276";
        } else if bytes[i..].starts_with(b"/*") {
            i += 2;
            let mut depth = 1;
            while i < bytes.len() && depth > 0 {
                if bytes[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                } else if bytes[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            color = "#738276";
        } else if bytes[i] == b'\'' || bytes[i] == b'"' {
            let quote = bytes[i];
            i += 1;
            while i < bytes.len() {
                if bytes[i] == quote {
                    i += 1;
                    if i < bytes.len() && bytes[i] == quote {
                        i += 1;
                    } else {
                        break;
                    }
                } else if quote == b'\'' && bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            color = if quote == b'\'' { "#238047" } else { "#94651c" };
        } else if bytes[i] == b'$'
            && sql[i + 1..].find('$').is_some_and(|end| {
                sql[i + 1..i + 1 + end]
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            })
        {
            let end = i + 2 + sql[i + 1..].find('$').unwrap();
            let delimiter = &sql[i..end];
            i = sql[end..]
                .find(delimiter)
                .map_or(sql.len(), |pos| end + pos + delimiter.len());
            color = "#238047";
        } else if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' || bytes[i] >= 128 {
            i += 1;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric()
                    || bytes[i] == b'_'
                    || bytes[i] >= 128
                    || bytes[i] == b'$')
            {
                i += 1;
            }
            color = if KEYWORDS
                .split_ascii_whitespace()
                .any(|word| word.eq_ignore_ascii_case(&sql[start..i]))
            {
                "#245dc1"
            } else {
                "#243447"
            };
        } else if bytes[i].is_ascii_digit() {
            i += 1;
            while i < bytes.len()
                && (bytes[i].is_ascii_digit() || matches!(bytes[i], b'.' | b'e' | b'E'))
            {
                i += 1;
            }
            color = "#8554a5";
        } else {
            i += 1;
            color = "#243447";
        }
        if let Some((range, _)) = result.last_mut().filter(|(_, previous)| *previous == color) {
            range.end = i;
        } else {
            result.push((start..i, color));
        }
    }
    result
}

pub fn highlight(sql: &str) -> Vec<Vec<crate::SqlRun>> {
    if sql.len() > 64 * 1024 || sql.bytes().filter(|b| *b == b'\n').count() >= 500 {
        return Vec::new();
    }
    let sql = sql.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines = vec![Vec::new()];
    for (range, color) in tokens(&sql) {
        for (index, part) in sql[range].split('\n').enumerate() {
            if index > 0 { lines.push(Vec::new()); }
            if !part.is_empty() {
                let rgb = u32::from_str_radix(&color[1..], 16).unwrap();
                let offset = lines.last().unwrap().last().map_or(0, |run: &crate::SqlRun| run.offset + run.text.len() as i32);
                lines.last_mut().unwrap().push(crate::SqlRun {
                    text: part.into(),
                    color: slint::Color::from_argb_encoded(0xff000000 | rgb),
                    offset,
                    source: Default::default(),
                });
            }
        }
    }
    for line in &mut lines {
        let source: String = line.iter().map(|run| run.text.as_str()).collect();
        let source = slint::SharedString::from(source);
        for run in line { run.source = source.clone(); }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lexer_preserves_unicode_literals_and_nested_comments() {
        let sql = "SELECT '中''文', $$a\nb$$, 42 /* a /* b */ c */ -- end";
        let parts = tokens(sql);
        assert_eq!(
            parts
                .iter()
                .map(|(range, _)| &sql[range.clone()])
                .collect::<String>(),
            sql
        );
        assert!(parts.iter().any(
            |(range, color)| &sql[range.clone()] == "/* a /* b */ c */" && *color == "#738276"
        ));
        assert!(
            parts
                .iter()
                .any(|(range, color)| &sql[range.clone()] == "$$a\nb$$" && *color == "#238047")
        );
        assert!(highlight(&"a".repeat(65537)).is_empty());
    }

    #[test]
    fn highlighted_runs_use_offsets_in_the_complete_line() {
        for line in highlight("SELECT '中文', 42 AS value\nGROUP BY 1, 2") {
            let text: String = line.iter().map(|run| run.text.as_str()).collect();
            let mut offset = 0;
            for run in line {
                assert_eq!(run.source.as_str(), text);
                assert_eq!(run.offset as usize, offset);
                assert!(text[offset..].starts_with(run.text.as_str()));
                offset += run.text.len();
            }
        }
    }
}
