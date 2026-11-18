//! Loading a CSV export into memory. Every cell is kept as text; an empty cell is `None`.

use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct Dataset {
    pub name: String,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    index: HashMap<String, usize>,
}

impl Dataset {
    pub fn new(name: impl Into<String>, headers: Vec<String>, rows: Vec<Vec<Option<String>>>) -> Self {
        let index = headers.iter().enumerate().map(|(i, h)| (h.clone(), i)).collect();
        Self { name: name.into(), headers, rows, index }
    }

    pub fn from_csv(path: &Path) -> Result<Self, String> {
        let mut reader = csv::ReaderBuilder::new()
            .flexible(true)
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
        Ok(Self::new(name, headers, rows))
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
