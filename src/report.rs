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
    for f in s.field_changes.iter().take(FIELDS_ON_SCREEN) {
        let field = format!("{} -> {}", f.source_field, f.target_field);
        writeln!(out, "    {field:<34}{:>8}  {:>6}%", f.changed, py_float(f.percent))?;
    }
    if s.field_changes.len() > FIELDS_ON_SCREEN {
        writeln!(out, "    and {} more fields in the JSON report", s.field_changes.len() - FIELDS_ON_SCREEN)?;
    }
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

const FIELDS_ON_SCREEN: usize = 8;
/// Findings shown in the Markdown report; the CSV and JSON carry the rest.
pub const MARKDOWN_FINDINGS: usize = 25;

/// Text that is safe inside a Markdown table cell.
fn md_cell(value: &str) -> String {
    let flat = value.replace(['\r', '\n'], " ");
    let escaped = flat.replace('\\', "\\\\").replace('|', "\\|").replace('`', "\\`").replace('<', "&lt;");
    if escaped.is_empty() {
        "—".into()
    } else {
        escaped
    }
}

/// A Markdown summary, written for a pull request comment or `$GITHUB_STEP_SUMMARY`.
pub fn markdown(out: &mut impl Write, outcome: &Outcome, meta: &Meta) -> std::io::Result<()> {
    let s = &outcome.summary;
    let verdict = if s.acceptance.passed { "✅ accepted" } else { "❌ rejected" };
    writeln!(out, "### driftcheck: {verdict}, quality {}", py_float(s.quality_score))?;
    writeln!(out)?;
    writeln!(
        out,
        "`{}` → `{}` · {} source rows, {} target rows, reconciled in {} ms",
        md_cell(meta.source),
        md_cell(meta.target),
        s.source_rows,
        s.target_rows,
        meta.elapsed_ms
    )?;
    writeln!(out)?;
    writeln!(out, "| Outcome | Records |")?;
    writeln!(out, "|---|---:|")?;
    for status in Status::ALL {
        writeln!(out, "| `{}` | {} |", status.as_str(), outcome.count(status))?;
    }
    if !s.acceptance.reasons.is_empty() {
        writeln!(out)?;
        writeln!(out, "**Why it was rejected**")?;
        writeln!(out)?;
        for reason in &s.acceptance.reasons {
            writeln!(out, "- {}", md_cell(reason))?;
        }
    }
    if !s.field_changes.is_empty() {
        writeln!(out)?;
        writeln!(out, "| Field | Rule | Records that differ | Share |")?;
        writeln!(out, "|---|---|---:|---:|")?;
        for f in &s.field_changes {
            writeln!(
                out,
                "| `{}` → `{}` | {} | {} | {}% |",
                md_cell(&f.source_field),
                md_cell(&f.target_field),
                f.rule,
                f.changed,
                py_float(f.percent)
            )?;
        }
    }
    let findings: Vec<&Record> = outcome.records.iter().filter(|r| r.status != Status::Matched).collect();
    if !findings.is_empty() {
        writeln!(out)?;
        writeln!(out, "<details><summary>First {} of {} findings</summary>", findings.len().min(MARKDOWN_FINDINGS), findings.len())?;
        writeln!(out)?;
        writeln!(out, "| Outcome | Source key | Target key | Field | Source value | Target value |")?;
        writeln!(out, "|---|---|---|---|---|---|")?;
        let opt = |v: &Option<String>| md_cell(v.as_deref().unwrap_or(""));
        for r in findings.iter().take(MARKDOWN_FINDINGS) {
            let first = r.diffs.first();
            writeln!(
                out,
                "| `{}` | {} | {} | {} | {} | {} |",
                r.status.as_str(),
                opt(&r.source_key),
                opt(&r.target_key),
                first.map(|d| md_cell(&format!("{} → {}", d.source_field, d.target_field))).unwrap_or_else(|| "—".into()),
                first.map(|d| opt(&d.source_value)).unwrap_or_else(|| "—".into()),
                first.map(|d| opt(&d.target_value)).unwrap_or_else(|| "—".into()),
            )?;
        }
        writeln!(out)?;
        writeln!(out, "</details>")?;
    }
    Ok(())
}

pub fn write_markdown(path: &Path, outcome: &Outcome, meta: &Meta) -> Result<(), String> {
    let err = |e: std::io::Error| format!("{}: {e}", path.display());
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(err)?;
    let mut w = std::io::BufWriter::new(file);
    markdown(&mut w, outcome, meta).map_err(err)?;
    w.flush().map_err(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_cells_cannot_break_the_table() {
        assert_eq!(md_cell("a|b"), "a\\|b");
        assert_eq!(md_cell("x\ny"), "x y");
        assert_eq!(md_cell("<script>"), "&lt;script>");
        assert_eq!(md_cell(""), "—");
    }

    #[test]
    fn markdown_report_names_the_verdict_and_the_worst_field() {
        use crate::config::Config;
        use crate::data::Dataset;
        let row = |v: &[&str]| v.iter().map(|c| Some(c.to_string())).collect::<Vec<_>>();
        let s = Dataset::new("a.csv", vec!["id".into(), "plan".into()], vec![row(&["1", "Pro"]), row(&["2", "Pro"])]);
        let t = Dataset::new("b.csv", vec!["id".into(), "plan".into()], vec![row(&["1", "Pro"]), row(&["2", "Free"])]);
        let config =
            Config::from_json(r#"{"source_key":"id","target_key":"id","mappings":[{"source":"plan","target":"plan"}]}"#)
                .unwrap();
        let o = crate::reconcile::reconcile(&s, &t, &config).unwrap();
        let mut md = Vec::new();
        markdown(&mut md, &o, &Meta { source: "a.csv", target: "b.csv", elapsed_ms: 1 }).unwrap();
        let md = String::from_utf8(md).unwrap();
        assert!(md.starts_with("### driftcheck: ❌ rejected, quality 50.0"), "{md}");
        assert!(md.contains("| `plan` → `plan` | exact | 1 | 50.0% |"), "{md}");
        assert!(md.contains("| `changed` | 2 | 2 | plan → plan | Pro | Free |"), "{md}");
    }

    #[test]
    fn formulas_are_neutralised() {
        assert_eq!(csv_safe("=HYPERLINK(\"x\")"), "'=HYPERLINK(\"x\")");
        assert_eq!(csv_safe("-12"), "'-12");
        assert_eq!(csv_safe("C-1001"), "C-1001");
    }
}
