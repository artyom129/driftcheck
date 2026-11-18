//! What a check leaves behind: a summary on the terminal, a JSON report and a CSV of findings.

use crate::reconcile::{py_float, Outcome, Record, Status};
use serde::Serialize;
use std::io::Write;
use std::path::Path;

#[derive(Serialize)]
struct JsonReport<'a> {
    tool: &'static str,
    version: &'static str,
    source: &'a str,
    target: &'a str,
    elapsed_ms: u128,
    summary: &'a crate::reconcile::Summary,
    #[serde(skip_serializing_if = "Option::is_none")]
    records: Option<&'a [Record]>,
}

pub struct Meta<'a> {
    pub source: &'a str,
    pub target: &'a str,
    pub elapsed_ms: u128,
}

pub fn write_json(path: &Path, outcome: &Outcome, meta: &Meta, with_records: bool) -> Result<(), String> {
    let report = JsonReport {
        tool: "driftcheck",
        version: env!("CARGO_PKG_VERSION"),
        source: meta.source,
        target: meta.target,
        elapsed_ms: meta.elapsed_ms,
        summary: &outcome.summary,
        records: with_records.then_some(outcome.records.as_slice()),
    };
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::to_writer_pretty(std::io::BufWriter::new(file), &report).map_err(|e| format!("{}: {e}", path.display()))
}

/// Keep spreadsheet apps from running a cell that starts like a formula.
pub fn csv_safe(value: &str) -> String {
    if value.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{value}")
    } else {
        value.to_string()
    }
}

/// One row per field difference, plus one row for every record that was not a
/// clean key match. Clean key matches are left out unless `all` is set.
pub fn write_csv(path: &Path, outcome: &Outcome, all: bool) -> Result<(), String> {
    let err = |e: csv::Error| format!("{}: {e}", path.display());
    let mut w = csv::Writer::from_path(path).map_err(err)?;
    w.write_record(["status", "source_key", "target_key", "match_score", "field", "source_value", "target_value", "rule"])
        .map_err(err)?;
    let opt = |v: &Option<String>| csv_safe(v.as_deref().unwrap_or(""));
    for r in &outcome.records {
        // Records found by name stay in: their key changed, which someone should see.
        if r.status == Status::Matched && !all {
            continue;
        }
        let score = format!("{:.2}", r.match_score);
        if r.diffs.is_empty() {
            w.write_record([r.status.as_str(), &opt(&r.source_key), &opt(&r.target_key), &score, "", "", "", ""])
                .map_err(err)?;
        }
        for d in &r.diffs {
            let field = format!("{} -> {}", d.source_field, d.target_field);
            w.write_record([
                r.status.as_str(),
                &opt(&r.source_key),
                &opt(&r.target_key),
                &score,
                &csv_safe(&field),
                &opt(&d.source_value),
                &opt(&d.target_value),
                d.rule,
            ])
            .map_err(err)?;
        }
    }
    w.flush().map_err(|e| format!("{}: {e}", path.display()))
}

pub fn print_summary(out: &mut impl Write, outcome: &Outcome, meta: &Meta, load_ms: u128) -> std::io::Result<()> {
    let s = &outcome.summary;
    writeln!(out, "driftcheck  {} -> {}", meta.source, meta.target)?;
    writeln!(
        out,
        "{} source rows, {} target rows, loaded in {} ms, reconciled in {} ms",
        s.source_rows, s.target_rows, load_ms, meta.elapsed_ms
    )?;
    writeln!(out)?;
    for status in Status::ALL {
        writeln!(out, "  {:<15}{:>10}", status.as_str(), outcome.count(status))?;
    }
    writeln!(out)?;
    writeln!(out, "  changed fields {:>10}", s.changed_fields)?;
    if !s.duplicate_source_keys.is_empty() {
        writeln!(out, "  repeated source keys: {}", s.duplicate_source_keys.len())?;
    }
    if !s.duplicate_target_keys.is_empty() {
        writeln!(out, "  repeated target keys: {}", s.duplicate_target_keys.len())?;
    }
    writeln!(out)?;
    let verdict = if s.acceptance.passed { "accepted" } else { "rejected" };
    writeln!(out, "quality {}  {}", py_float(s.quality_score), verdict)?;
    for reason in &s.acceptance.reasons {
        writeln!(out, "  - {reason}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formulas_are_neutralised() {
        assert_eq!(csv_safe("=HYPERLINK(\"x\")"), "'=HYPERLINK(\"x\")");
        assert_eq!(csv_safe("-12"), "'-12");
        assert_eq!(csv_safe("C-1001"), "C-1001");
    }
}
