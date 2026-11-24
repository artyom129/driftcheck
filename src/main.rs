use clap::{Parser, Subcommand};
use driftcheck::config::Config;
use driftcheck::data::{parse_delimiter, Dataset};
use driftcheck::report::{self, Meta};
use driftcheck::{gen, infer, patch, reconcile};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

/// Exit codes, so a check can gate a pipeline.
const ACCEPTED: u8 = 0;
const REJECTED: u8 = 1;
const ERROR: u8 = 2;

#[derive(Parser)]
#[command(name = "driftcheck", version, about = "Compare two exports record by record and get a verdict")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Reconcile a source export against a target export.
    Check {
        /// The data you migrated from (CSV)
        #[arg(long)]
        source: PathBuf,
        /// The data you migrated to (CSV)
        #[arg(long)]
        target: PathBuf,
        /// Mapping contract (JSON, the same format SyncProof uses)
        #[arg(long)]
        contract: PathBuf,
        /// Write a JSON report here
        #[arg(long)]
        json: Option<PathBuf>,
        /// Include every record in the JSON report, not only the summary
        #[arg(long)]
        records: bool,
        /// Write the findings as CSV here
        #[arg(long)]
        csv: Option<PathBuf>,
        /// Also list clean key matches in the CSV
        #[arg(long)]
        all: bool,
        /// Append a Markdown summary here, for example to $GITHUB_STEP_SUMMARY
        #[arg(long)]
        markdown: Option<PathBuf>,
        /// Write a SQL patch plan that makes the target match the source
        #[arg(long)]
        patch: Option<PathBuf>,
        /// Table name used in the patch plan, schema.table is allowed
        #[arg(long, default_value = "target")]
        table: String,
        /// Field delimiter of both files: a character or comma, semicolon, tab, pipe (default: detected)
        #[arg(long)]
        delimiter: Option<String>,
        /// Worker threads (default: one per core)
        #[arg(long)]
        threads: Option<usize>,
        /// Print nothing; rely on the exit code and the reports
        #[arg(long)]
        quiet: bool,
    },
    /// Draft a mapping contract by looking at what the two exports hold.
    Init {
        /// The data you migrated from (CSV)
        #[arg(long)]
        source: PathBuf,
        /// The data you migrated to (CSV)
        #[arg(long)]
        target: PathBuf,
        /// Where to write the contract
        #[arg(long, default_value = "mapping.json")]
        out: PathBuf,
        /// Replace the file if it exists
        #[arg(long)]
        force: bool,
        /// Field delimiter of both files (default: detected)
        #[arg(long)]
        delimiter: Option<String>,
    },
    /// Generate a test migration of any size with a known number of defects.
    Gen {
        #[arg(long, default_value_t = 1200)]
        rows: usize,
        #[arg(long, default_value_t = 42)]
        seed: u64,
        /// Directory for legacy.csv, migrated.csv, mapping.json and expected.json
        #[arg(long)]
        out: PathBuf,
    },
}

fn load_pair(source: &Path, target: &Path, delimiter: Option<&str>) -> Result<(Dataset, Dataset), String> {
    let delimiter = delimiter.map(parse_delimiter).transpose()?;
    let (s, t) = rayon::join(|| Dataset::from_csv_with(source, delimiter), || Dataset::from_csv_with(target, delimiter));
    Ok((s?, t?))
}

fn run(cli: Cli) -> Result<u8, String> {
    match cli.command {
        Command::Gen { rows, seed, out } => {
            if rows == 0 {
                return Err("--rows must be at least 1".into());
            }
            let generated = gen::build(rows, seed);
            gen::write(&out, &generated)?;
            println!("{}", serde_json::to_string_pretty(&generated.expected).map_err(|e| e.to_string())?);
            Ok(ACCEPTED)
        }
        Command::Init { source, target, out, force, delimiter } => {
            if out.exists() && !force {
                return Err(format!("{} already exists, add --force to replace it", out.display()));
            }
            let (s, t) = load_pair(&source, &target, delimiter.as_deref())?;
            let draft = infer::draft(&s, &t)?;
            let json = serde_json::to_string_pretty(&draft.config).map_err(|e| e.to_string())?;
            std::fs::write(&out, json + "\n").map_err(|e| format!("{}: {e}", out.display()))?;

            println!("driftcheck init  {} -> {}", s.name, t.name);
            println!();
            println!("  key  {} -> {}", draft.key.source, draft.key.target);
            for p in &draft.pairs {
                let rule = draft.config.mappings.iter().find(|m| m.source == p.source).map(|m| m.rule.as_str());
                println!(
                    "       {:<22} -> {:<22} {:<18} values {:>5.1}%",
                    p.source,
                    p.target,
                    rule.unwrap_or(""),
                    p.value_overlap * 100.0
                );
            }
            let fuzzy: Vec<&str> = draft.config.fuzzy_fields.iter().map(|m| m.source.as_str()).collect();
            if !fuzzy.is_empty() {
                println!("  renamed records are searched by: {}", fuzzy.join(", "));
            }
            if !draft.unpaired_source.is_empty() {
                println!("  not paired in the source: {}", draft.unpaired_source.join(", "));
            }
            if !draft.unpaired_target.is_empty() {
                println!("  not paired in the target: {}", draft.unpaired_target.join(", "));
            }
            println!();
            println!("wrote {}. Read it, adjust the rules and limits, then run driftcheck check.", out.display());
            Ok(ACCEPTED)
        }
        Command::Check {
            source,
            target,
            contract,
            json,
            records,
            csv,
            all,
            markdown,
            patch: patch_path,
            table,
            delimiter,
            threads,
            quiet,
        } => {
            if let Some(n) = threads {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(n.max(1))
                    .build_global()
                    .map_err(|e| e.to_string())?;
            }
            let text = std::fs::read_to_string(&contract).map_err(|e| format!("{}: {e}", contract.display()))?;
            let config = Config::from_json(&text)?;

            let loading = Instant::now();
            let (s, t) = load_pair(&source, &target, delimiter.as_deref())?;
            let load_ms = loading.elapsed().as_millis();

            let started = Instant::now();
            let outcome = reconcile::reconcile(&s, &t, &config)?;
            let meta = Meta {
                source: &s.name,
                target: &t.name,
                elapsed_ms: started.elapsed().as_millis(),
            };

            if let Some(path) = &json {
                report::write_json(path, &outcome, &meta, records)?;
            }
            if let Some(path) = &csv {
                report::write_csv(path, &outcome, all)?;
            }
            if let Some(path) = &markdown {
                report::write_markdown(path, &outcome, &meta)?;
            }
            let plan = match &patch_path {
                Some(path) => Some(patch::write(path, &outcome, &s, &t, &config, &table)?),
                None => None,
            };
            if !quiet {
                let mut stdout = std::io::stdout().lock();
                report::print_summary(&mut stdout, &outcome, &meta, load_ms).map_err(|e| e.to_string())?;
                if let (Some(n), Some(path)) = (plan, &patch_path) {
                    use std::io::Write;
                    writeln!(
                        stdout,
                        "\npatch plan {}: {} updates, {} inserts, {} deletes to review, {} skipped",
                        path.display(),
                        n.updates,
                        n.inserts,
                        n.deletes_for_review,
                        n.skipped
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
            Ok(if outcome.summary.acceptance.passed { ACCEPTED } else { REJECTED })
        }
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => ExitCode::from(code),
        Err(message) => {
            eprintln!("driftcheck: {message}");
            ExitCode::from(ERROR)
        }
    }
}
