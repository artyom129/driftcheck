//! Loading a CSV export into memory. Every cell is kept as text; an empty cell is `None`.

use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct Dataset {
    pub name: String,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub delimiter: u8,
    index: HashMap<String, usize>,
}

impl Dataset {
    pub fn new(name: impl Into<String>, headers: Vec<String>, rows: Vec<Vec<Option<String>>>) -> Self {
        let index = headers.iter().enumerate().map(|(i, h)| (h.clone(), i)).collect();
        Self { name: name.into(), headers, rows, delimiter: b',', index }
    }

    pub fn from_csv(path: &Path) -> Result<Self, String> {
        Self::from_csv_with(path, None)
    }

    /// Loads a CSV. Without an explicit delimiter it is read from the header line,
    /// so `;` exports from European spreadsheet settings load as they are.
    pub fn from_csv_with(path: &Path, delimiter: Option<u8>) -> Result<Self, String> {
        let delimiter = match delimiter {
            Some(d) => d,
            None => sniff_delimiter(path)?,
        };
        let mut reader = csv::ReaderBuilder::new()
            .flexible(true)
            .delimiter(delimiter)
            .from_path(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let headers: Vec<String> = reader
            .headers()
            .map_err(|e| format!("{}: {e}", path.display()))?
            .iter()
            .map(|h| h.trim_start_matches('\u{feff}').to_string())
            .collect();
        let width = headers.len();
        let mut rows = Vec::new();
        for (n, record) in reader.records().enumerate() {
            let record = record.map_err(|e| format!("{}: row {}: {e}", path.display(), n + 2))?;
            let mut row: Vec<Option<String>> = record
                .iter()
                .take(width)
                .map(|cell| if cell.is_empty() { None } else { Some(cell.to_string()) })
                .collect();
            row.resize(width, None);
            rows.push(row);
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut dataset = Self::new(name, headers, rows);
        dataset.delimiter = delimiter;
        Ok(dataset)
    }

    pub fn column(&self, name: &str) -> Option<usize> {
        self.index.get(name).copied()
    }

    pub fn require(&self, names: &[&str], side: &str) -> Result<Vec<usize>, String> {
        let mut missing: Vec<&str> = names.iter().copied().filter(|n| self.column(n).is_none()).collect();
        missing.sort_unstable();
        missing.dedup();
        if !missing.is_empty() {
            return Err(format!("columns not found in {side} dataset '{}': {}", self.name, missing.join(", ")));
        }
        Ok(names.iter().map(|n| self.column(n).unwrap()).collect())
    }

    #[inline]
    pub fn cell(&self, row: usize, col: usize) -> Option<&str> {
        self.rows[row][col].as_deref()
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

const CANDIDATES: [u8; 4] = [b',', b';', b'\t', b'|'];

/// The delimiter that splits the header line into the most columns, counting
/// only characters outside quotes. A file with one column reads as comma.
pub fn detect_delimiter(header_line: &str) -> u8 {
    let mut counts = [0usize; CANDIDATES.len()];
    let mut quoted = false;
    for b in header_line.bytes() {
        if b == b'"' {
            quoted = !quoted;
        } else if !quoted {
            if let Some(i) = CANDIDATES.iter().position(|&c| c == b) {
                counts[i] += 1;
            }
        }
    }
    let mut best = 0;
    for i in 1..CANDIDATES.len() {
        if counts[i] > counts[best] {
            best = i;
        }
    }
    if counts[best] == 0 {
        b','
    } else {
        CANDIDATES[best]
    }
}

fn sniff_delimiter(path: &Path) -> Result<u8, String> {
    use std::io::{BufRead, BufReader};
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut first = Vec::new();
    BufReader::new(file)
        .read_until(b'\n', &mut first)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(detect_delimiter(&String::from_utf8_lossy(&first)))
}

/// Parses `--delimiter`: one ASCII character, or the words comma, semicolon, tab, pipe.
pub fn parse_delimiter(text: &str) -> Result<u8, String> {
    match text {
        "comma" => Ok(b','),
        "semicolon" => Ok(b';'),
        "tab" | "\\t" => Ok(b'\t'),
        "pipe" => Ok(b'|'),
        t if t.len() == 1 && t.is_ascii() => Ok(t.as_bytes()[0]),
        _ => Err(format!("--delimiter must be one character or comma, semicolon, tab, pipe (got '{text}')")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delimiter_comes_from_the_header() {
        assert_eq!(detect_delimiter("id,name,mrr\n"), b',');
        assert_eq!(detect_delimiter("id;name;mrr\r\n"), b';');
        assert_eq!(detect_delimiter("id\tname\tmrr"), b'\t');
        assert_eq!(detect_delimiter("id|name"), b'|');
        assert_eq!(detect_delimiter("only"), b',');
        // A comma inside a quoted header does not count.
        assert_eq!(detect_delimiter("\"name, full\";id;mrr"), b';');
    }

    #[test]
    fn delimiter_option() {
        assert_eq!(parse_delimiter("semicolon").unwrap(), b';');
        assert_eq!(parse_delimiter("tab").unwrap(), b'\t');
        assert_eq!(parse_delimiter(":").unwrap(), b':');
        assert!(parse_delimiter("ab").is_err());
    }

    #[test]
    fn semicolon_file_loads_without_options() {
        let dir = std::env::temp_dir().join(format!("driftcheck-delim-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("eu.csv");
        std::fs::write(&path, "id;name;mrr\nC-1;Ava;1.290,00\n").unwrap();
        let d = Dataset::from_csv(&path).unwrap();
        assert_eq!(d.headers, vec!["id", "name", "mrr"]);
        assert_eq!(d.cell(0, 2), Some("1.290,00"));
        assert_eq!(d.delimiter, b';');
        std::fs::remove_dir_all(&dir).ok();
    }
}
