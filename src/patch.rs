//! A SQL patch plan that brings the target in line with the source.
//!
//! The plan is written for review, not for blind execution:
//! - `changed` records get an `UPDATE` of exactly the fields that differ;
//! - `source_only` records get an `INSERT` of the key and every mapped field;
//! - `target_only` records get a `DELETE` that is commented out, because
//!   removing data is a decision for a person;
//! - renamed and ambiguous records are listed as comments.
//!
//! Everything runs in one transaction. Values are written as text literals, so
//! the target database converts them with its own rules.

use crate::config::Config;
use crate::data::Dataset;
use crate::reconcile::{Outcome, Record, Status};
use crate::value::norm_text;
use std::collections::HashSet;
use std::io::Write;
use std::path::Path;

/// `"name"`, with inner quotes doubled. `schema.table` is quoted part by part.
pub fn ident(name: &str) -> String {
    name.split('.').map(|part| format!("\"{}\"", part.replace('"', "\"\""))).collect::<Vec<_>>().join(".")
}

/// `'text'`, with inner quotes doubled; a missing value is `NULL`.
pub fn literal(value: Option<&str>) -> String {
    match value {
        None => "NULL".into(),
        Some(v) => format!("'{}'", v.replace('\'', "''")),
    }
}

/// Comments never span lines, so a value cannot end a comment early.
fn one_line(text: &str) -> String {
    text.replace(['\r', '\n'], " ")
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PlanCounts {
    pub updates: usize,
    pub inserts: usize,
    pub deletes_for_review: usize,
    pub skipped: usize,
}

pub fn build(
    out: &mut impl Write,
    outcome: &Outcome,
    source: &Dataset,
    target: &Dataset,
    config: &Config,
    table: &str,
) -> std::io::Result<PlanCounts> {
    let mut n = PlanCounts::default();
    let t = ident(table);
    let key = ident(&config.target_key);
    let s_key = source.column(&config.source_key);
    let t_key = target.column(&config.target_key);
    let repeated: HashSet<&str> = outcome.summary.duplicate_source_keys.iter().map(String::as_str).collect();
    let target_cell = |r: &Record| r.target_row.zip(t_key).and_then(|(row, col)| target.cell(row, col));
    let source_cell = |r: &Record| r.source_row.zip(s_key).and_then(|(row, col)| source.cell(row, col));

    writeln!(out, "-- driftcheck {} patch plan: {} -> {}", env!("CARGO_PKG_VERSION"), source.name, target.name)?;
    writeln!(out, "-- Makes {} match {}. Read it before running it.", one_line(table), one_line(&source.name))?;
    writeln!(out, "BEGIN;")?;

    let section = |out: &mut dyn Write, title: &str| writeln!(out, "\n-- {title}");

    section(out, "Changed records: only the fields that differ")?;
    for r in outcome.records.iter().filter(|r| r.status == Status::Changed) {
        let Some(addr) = target_cell(r) else {
            n.skipped += 1;
            writeln!(out, "-- skipped: a changed record has no target key")?;
            continue;
        };
        let sets: Vec<String> =
            r.diffs.iter().map(|d| format!("{} = {}", ident(&d.target_field), literal(d.source_value.as_deref()))).collect();
        writeln!(out, "UPDATE {t} SET {} WHERE {key} = {};", sets.join(", "), literal(Some(addr)))?;
        n.updates += 1;
    }

    section(out, "Missing in the target")?;
    let mut columns = vec![key.clone()];
    columns.extend(config.mappings.iter().map(|m| ident(&m.target)));
    let map_cols: Vec<Option<usize>> = config.mappings.iter().map(|m| source.column(&m.source)).collect();
    for r in outcome.records.iter().filter(|r| r.status == Status::SourceOnly) {
        let Some(k) = source_cell(r).filter(|k| !k.trim().is_empty()) else {
            n.skipped += 1;
            writeln!(out, "-- skipped: a source row without a key")?;
            continue;
        };
        if repeated.contains(norm_text(Some(k)).as_str()) {
            n.skipped += 1;
            writeln!(out, "-- skipped: key {} appears more than once in the source", one_line(k))?;
            continue;
        }
        let row = r.source_row.expect("source-only records come from a source row");
        let mut values = vec![literal(Some(k))];
        values.extend(map_cols.iter().map(|c| literal(c.and_then(|c| source.cell(row, c)))));
        writeln!(out, "INSERT INTO {t} ({}) VALUES ({});", columns.join(", "), values.join(", "))?;
        n.inserts += 1;
    }

    section(out, "Only in the target: remove the comment marks after checking each one")?;
    for r in outcome.records.iter().filter(|r| r.status == Status::TargetOnly) {
        match target_cell(r).filter(|k| !k.trim().is_empty()) {
            Some(k) => {
                writeln!(out, "-- DELETE FROM {t} WHERE {key} = {};", one_line(&literal(Some(k))))?;
                n.deletes_for_review += 1;
            }
            None => {
                n.skipped += 1;
                writeln!(out, "-- skipped: a target row without a key")?;
            }
        }
    }

    section(out, "Found under a new key, for information")?;
    for r in outcome.records.iter().filter(|r| r.status == Status::FuzzyMatched) {
        writeln!(
            out,
            "-- {} is {} in the target (similarity {:.2})",
            one_line(r.source_key.as_deref().unwrap_or("?")),
            one_line(r.target_key.as_deref().unwrap_or("?")),
            r.match_score
        )?;
    }
    for r in outcome.records.iter().filter(|r| r.status == Status::Ambiguous) {
        n.skipped += 1;
        writeln!(
            out,
            "-- ambiguous: {} has two equally close candidates, decide by hand",
            one_line(r.source_key.as_deref().unwrap_or("?"))
        )?;
    }

    writeln!(out, "\nCOMMIT;")?;
    writeln!(
        out,
        "-- {} updates, {} inserts, {} deletes to review, {} skipped",
        n.updates, n.inserts, n.deletes_for_review, n.skipped
    )?;
    Ok(n)
}

pub fn write(
    path: &Path,
    outcome: &Outcome,
    source: &Dataset,
    target: &Dataset,
    config: &Config,
    table: &str,
) -> Result<PlanCounts, String> {
    let err = |e: std::io::Error| format!("{}: {e}", path.display());
    let file = std::fs::File::create(path).map_err(err)?;
    let mut w = std::io::BufWriter::new(file);
    let counts = build(&mut w, outcome, source, target, config, table).map_err(err)?;
    w.flush().map_err(err)?;
    Ok(counts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reconcile::reconcile;

    fn ds(name: &str, headers: &[&str], rows: &[&[&str]]) -> Dataset {
        Dataset::new(
            name,
            headers.iter().map(|h| h.to_string()).collect(),
            rows.iter()
                .map(|r| r.iter().map(|c| if c.is_empty() { None } else { Some(c.to_string()) }).collect())
                .collect(),
        )
    }

    #[test]
    fn quoting() {
        assert_eq!(ident("crm.customers"), "\"crm\".\"customers\"");
        assert_eq!(ident("we\"ird"), "\"we\"\"ird\"");
        assert_eq!(literal(Some("O'Brien")), "'O''Brien'");
        assert_eq!(literal(None), "NULL");
    }

    #[test]
    fn a_plan_for_every_outcome() {
        let s = ds("legacy.csv", &["id", "name", "mrr"], &[
            &["C-1", "Ava Morgan", "129"],
            &["C-2", "Noah O'Brien", "49"],
            &["C-3", "Mia Chen", "299"],
            &["C-3", "Mia Chen", "299"],
            &["C-4", "Liam Garcia", ""],
        ]);
        let t = ds("new.csv", &["key", "full_name", "revenue"], &[
            &["C-1", "Ava Morgan", "129"],
            &["C-2", "Noah O'Brien", "39"],
            &["C-3", "Mia Chen", "299"],
            &["C-9", "Old Account", "1"],
        ]);
        let config = Config::from_json(
            r#"{"source_key":"id","target_key":"key","mappings":[
                {"source":"name","target":"full_name","rule":"text"},
                {"source":"mrr","target":"revenue","rule":"numeric"}]}"#,
        )
        .unwrap();
        let o = reconcile(&s, &t, &config).unwrap();
        let mut sql = Vec::new();
        let n = build(&mut sql, &o, &s, &t, &config, "crm.customers").unwrap();
        let sql = String::from_utf8(sql).unwrap();

        assert!(sql.contains(r#"UPDATE "crm"."customers" SET "revenue" = '49' WHERE "key" = 'C-2';"#), "{sql}");
        assert!(sql.contains(
            r#"INSERT INTO "crm"."customers" ("key", "full_name", "revenue") VALUES ('C-4', 'Liam Garcia', NULL);"#
        ));
        assert!(sql.contains(r#"-- DELETE FROM "crm"."customers" WHERE "key" = 'C-9';"#));
        assert!(sql.contains("key C-3 appears more than once"));
        assert!(sql.trim_start().starts_with("-- driftcheck"));
        assert!(sql.contains("BEGIN;") && sql.contains("COMMIT;"));
        assert_eq!(n, PlanCounts { updates: 1, inserts: 1, deletes_for_review: 1, skipped: 1 });
    }

    #[test]
    fn a_value_cannot_break_out_of_a_comment() {
        let s = ds("s", &["id", "name"], &[&["A\n1; DROP TABLE x", "a"]]);
        let t = ds("t", &["id", "name"], &[&["B\nDROP", "zzz"]]);
        let config = Config::from_json(
            r#"{"source_key":"id","target_key":"id","mappings":[{"source":"name","target":"name"}]}"#,
        )
        .unwrap();
        let o = reconcile(&s, &t, &config).unwrap();
        let mut sql = Vec::new();
        build(&mut sql, &o, &s, &t, &config, "t").unwrap();
        let sql = String::from_utf8(sql).unwrap();
        for line in sql.lines() {
            assert!(!line.starts_with("DROP"), "{sql}");
        }
    }
}
