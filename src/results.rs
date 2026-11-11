use std::{
    collections::BTreeSet,
    fs::File,
    io::{BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::Path,
};

pub const PAGE_SIZE: usize = 100;
pub const MAX_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_ROW: usize = 4 * 1024 * 1024;
pub type Cells = Vec<Option<String>>;

fn number_key(value: &str) -> Option<(i8, i64, String)> {
    let value = value.trim();
    let negative = value.starts_with('-');
    let value = value.strip_prefix(['-', '+']).unwrap_or(value);
    let (mantissa, exponent) = match value.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i64>().ok()?),
        None => (value, 0),
    };
    let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if integer.is_empty() && fraction.is_empty() { return None; }
    if !integer.bytes().chain(fraction.bytes()).all(|byte| byte.is_ascii_digit()) { return None; }
    let digits = format!("{integer}{fraction}");
    let significant = digits.trim_start_matches('0');
    if significant.is_empty() { return Some((0, 0, String::new())); }
    let magnitude = exponent.checked_add(integer.len() as i64)?.checked_sub((digits.len() - significant.len()) as i64)?;
    let significant = significant.trim_end_matches('0');
    if negative {
        let mut complement: String = significant.bytes().map(|byte| (b'9' - (byte - b'0')) as char).collect();
        complement.push(':');
        Some((-1, magnitude.checked_neg()?, complement))
    } else {
        Some((1, magnitude, significant.into()))
    }
}

pub struct ResultSet {
    pub repeats: usize,
    pub status: bool,
    pub exported: bool,
    pub headers: Vec<String>,
    pub count: usize,
    file: File,
    writer: BufWriter<File>,
    offsets: Vec<u64>,
    bytes: u64,
}

impl ResultSet {
    pub fn new(headers: Vec<String>) -> Result<Self, String> {
        let file = tempfile::tempfile().map_err(|e| e.to_string())?;
        let writer = BufWriter::new(file.try_clone().map_err(|e| e.to_string())?);
        Ok(Self {
            repeats: 1,
            status: false,
            exported: false,
            headers,
            count: 0,
            file,
            writer,
            offsets: Vec::new(),
            bytes: 0,
        })
    }

    pub fn push(&mut self, row: Cells) -> Result<u64, String> {
        if row.iter().flatten().map(String::len).sum::<usize>() > MAX_ROW {
            return Err("LIMIT_ROW".into());
        }
        let data = serde_json::to_vec(&row).map_err(|e| e.to_string())?;
        if data.len() > MAX_ROW {
            return Err("LIMIT_ROW".into());
        }
        if self.count.is_multiple_of(PAGE_SIZE) {
            self.offsets.push(self.bytes);
        }
        self.writer
            .write_all(&data)
            .and_then(|_| self.writer.write_all(b"\n"))
            .map_err(|e| e.to_string())?;
        self.bytes += data.len() as u64 + 1;
        self.count += 1;
        Ok(data.len() as u64 + 1)
    }

    pub fn finish(&mut self) -> Result<(), String> {
        self.writer.flush().map_err(|e| e.to_string())
    }

    pub fn same_content(&mut self, other: &mut Self) -> Result<bool, String> {
        if self.headers != other.headers || self.count != other.count || self.bytes != other.bytes { return Ok(false); }
        self.finish()?;
        other.finish()?;
        self.file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        other.file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        let mut left = [0; 8192];
        let mut right = [0; 8192];
        loop {
            let count = self.file.read(&mut left).map_err(|e| e.to_string())?;
            other.file.read_exact(&mut right[..count]).map_err(|e| e.to_string())?;
            if left[..count] != right[..count] { return Ok(false); }
            if count == 0 { return Ok(true); }
        }
    }

    pub fn sort(&mut self, column: usize, descending: bool, cancelled: impl Fn() -> bool) -> Result<(), String> {
        if column >= self.headers.len() { return Err("Invalid column".into()); }
        self.finish()?;
        let mut source = self.file.try_clone().map_err(|e| e.to_string())?;
        source.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        let mut reader = BufReader::new(source);
        let mut entries = Vec::with_capacity(self.count);
        let mut offset = 0;
        let mut line = String::new();
        loop {
            if cancelled() { return Err("CANCELLED".into()); }
            line.clear();
            let length = reader.read_line(&mut line).map_err(|e| e.to_string())?;
            if length == 0 { break; }
            let mut row: Cells = serde_json::from_str(&line).map_err(|e| e.to_string())?;
            entries.push((row[column].take(), offset, length));
            offset += length as u64;
        }
        let numeric = entries.iter().filter_map(|entry| entry.0.as_deref()).all(|value| number_key(value).is_some());
        entries.sort_by_cached_key(|entry| {
            let text = entry.0.as_deref();
            (text.is_some(), if numeric { text.and_then(number_key) } else { None }, if numeric { None } else { entry.0.clone() })
        });
        if descending { entries.reverse(); }
        let mut sorted = Self::new(self.headers.clone())?;
        sorted.repeats = self.repeats;
        sorted.status = self.status;
        sorted.exported = self.exported;
        let mut buffer = Vec::new();
        for (_, offset, length) in entries {
            if cancelled() { return Err("CANCELLED".into()); }
            reader.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
            buffer.resize(length, 0);
            reader.read_exact(&mut buffer).map_err(|e| e.to_string())?;
            if sorted.count.is_multiple_of(PAGE_SIZE) { sorted.offsets.push(sorted.bytes); }
            sorted.writer.write_all(&buffer).map_err(|e| e.to_string())?;
            sorted.count += 1;
            sorted.bytes += length as u64;
        }
        sorted.finish()?;
        *self = sorted;
        Ok(())
    }

    pub fn page(
        &mut self,
        page: usize,
        preview: bool,
        selected: Option<usize>,
    ) -> Result<Vec<Cells>, String> {
        self.finish()?;
        let Some(offset) = self.offsets.get(page) else {
            return Ok(Vec::new());
        };
        let mut file = self.file.try_clone().map_err(|e| e.to_string())?;
        file.seek(SeekFrom::Start(*offset))
            .map_err(|e| e.to_string())?;
        let mut bytes = 0;
        BufReader::new(file)
            .lines()
            .take(PAGE_SIZE)
            .skip(selected.unwrap_or(0))
            .take(if selected.is_some() { 1 } else { PAGE_SIZE })
            .map(|line| {
                let mut row: Cells = serde_json::from_str(&line.map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
                for text in row.iter_mut().flatten() {
                    if preview {
                        let limit = if bytes < 2 * 1024 * 1024 { 300 } else { 0 };
                        if let Some((offset, _)) = text.char_indices().nth(limit) {
                            text.truncate(offset);
                            text.push('…');
                        }
                        text.shrink_to_fit();
                    }
                    bytes += text.len();
                }
                if !preview && bytes > 8 * 1024 * 1024 {
                    return Err("LIMIT_COPY".into());
                }
                Ok(row)
            })
            .collect()
    }

    pub fn clipboard(&mut self, page: Option<usize>, selected: Option<&[usize]>, selected_cells: Option<&[(usize, usize)]>, include_headers: bool, cancelled: impl Fn() -> bool) -> Result<String, String> {
        self.finish()?;
        let mut source = self.file.try_clone().map_err(|e| e.to_string())?;
        let offset = page.and_then(|page| self.offsets.get(page)).copied().unwrap_or(0);
        source.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
        let mut writer = csv::WriterBuilder::new().delimiter(b'\t').from_writer(Vec::new());
        let cells = selected_cells.map(|cells| cells.iter().copied().collect::<BTreeSet<_>>());
        let columns: Vec<usize> = match &cells {
            Some(cells) => cells.iter().map(|(_, column)| *column).collect::<BTreeSet<_>>().into_iter().collect(),
            None => (0..self.headers.len()).collect(),
        };
        if include_headers { writer.write_record(columns.iter().map(|column| self.headers[*column].as_str())).map_err(|e| e.to_string())?; }
        for (index, line) in BufReader::new(source).lines().take(if page.is_some() { PAGE_SIZE } else { usize::MAX }).enumerate() {
            if cancelled() { return Err("CANCELLED".into()); }
            if selected.is_some_and(|selected| !selected.contains(&index)) { continue; }
            let row: Cells = serde_json::from_str(&line.map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            writer.write_record(columns.iter().map(|column| {
                if cells.as_ref().is_some_and(|cells| !cells.contains(&(index, *column))) { "" }
                else { row[*column].as_deref().unwrap_or("NULL") }
            })).map_err(|e| e.to_string())?;
            if writer.get_ref().len() > 8 * 1024 * 1024 { return Err("LIMIT_COPY".into()); }
        }
        let data = writer.into_inner().map_err(|e| e.to_string())?;
        if data.len() > 8 * 1024 * 1024 { return Err("LIMIT_COPY".into()); }
        String::from_utf8(data).map_err(|e| e.to_string())
    }

    pub fn write_json_rows(&mut self, path: &Path) -> Result<(), String> {
        self.finish()?;
        self.file.seek(SeekFrom::Start(0)).map_err(|error| error.to_string())?;
        let mut target = tempfile::NamedTempFile::new_in(path.parent().ok_or("Invalid result path")?).map_err(|error| error.to_string())?;
        std::io::copy(&mut self.file, &mut target).map_err(|error| error.to_string())?;
        target.as_file().sync_all().map_err(|error| error.to_string())?;
        target.persist(path).map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn export(&mut self, path: &Path, cancelled: impl Fn() -> bool) -> Result<(), String> {
        self.finish()?;
        let dir = path.parent().ok_or("Invalid export path")?;
        let mut temp = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
        {
            temp.write_all(b"\xef\xbb\xbf").map_err(|e| e.to_string())?;
            let mut writer = csv::Writer::from_writer(&mut temp);
            writer
                .write_record(&self.headers)
                .map_err(|e| e.to_string())?;
            let mut source = self.file.try_clone().map_err(|e| e.to_string())?;
            source.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
            for line in BufReader::new(source).lines() {
                if cancelled() {
                    return Err("CANCELLED".into());
                }
                let row: Cells = serde_json::from_str(&line.map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
                writer
                    .write_record(row.iter().map(|v| v.as_deref().unwrap_or("NULL")))
                    .map_err(|e| e.to_string())?;
            }
            writer.flush().map_err(|e| e.to_string())?;
        }
        temp.as_file().sync_all().map_err(|e| e.to_string())?;
        temp.persist(path).map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorting_spans_pages_and_preserves_precise_values() {
        let mut set = ResultSet::new(vec!["value".into()]).unwrap();
        for i in (0..201).rev() { set.push(vec![Some(i.to_string())]).unwrap(); }
        set.sort(0, false, || false).unwrap();
        assert_eq!(set.page(1, false, Some(0)).unwrap()[0][0].as_deref(), Some("100"));
        set.sort(0, true, || false).unwrap();
        assert_eq!(set.page(2, false, None).unwrap()[0][0].as_deref(), Some("0"));
        assert!(set.sort(0, false, || true).is_err());
        assert_eq!(set.page(0, false, Some(0)).unwrap()[0][0].as_deref(), Some("200"));
        let values = ["-100", "-1.21", "-1.2", "-0.01", "0", "0.001", "1.2", "1.21", "9007199254740992", "9007199254740993", "1e20"];
        for pair in values.windows(2) { assert!(number_key(pair[0]).unwrap() < number_key(pair[1]).unwrap()); }
        assert_eq!(number_key("1.200"), number_key("12e-1"));
    }

    #[test]
    fn paging_and_export_preserve_null_unicode_and_newlines() {
        let mut set = ResultSet::new(vec!["值".into(), "empty".into()]).unwrap();
        for i in 0..201 {
            set.push(vec![Some(format!("{i},中\n文")), None]).unwrap();
        }
        assert_eq!(
            set.page(2, false, None).unwrap(),
            vec![vec![Some("200,中\n文".into()), None]]
        );
        assert_eq!(
            set.page(0, false, Some(1)).unwrap(),
            vec![vec![Some("1,中\n文".into()), None]]
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.csv");
        set.export(&path, || false).unwrap();
        let mut reader = csv::Reader::from_path(&path).unwrap();
        let rows: Vec<_> = reader.records().map(Result::unwrap).collect();
        assert_eq!(rows.len(), 201);
        assert_eq!(&rows[200][0], "200,中\n文");
        assert_eq!(&rows[200][1], "NULL");
        assert!(set.clipboard(Some(2), None, None, false, || false).unwrap().contains("\tNULL"));
        let original = std::fs::read(&path).unwrap();
        assert!(set.export(&path, || true).is_err());
        assert_eq!(std::fs::read(path).unwrap(), original);
    }
}
