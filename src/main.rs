use clap::{Parser, Subcommand};
use driftcheck::config::Config;
use driftcheck::data::Dataset;
use driftcheck::report::{self, Meta};
use driftcheck::{gen, reconcile};
use std::path::PathBuf;
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
        /// Worker threads (default: one per core)
        #[arg(long)]
        threads: Option<usize>,
        /// Print nothing; rely on the exit code and the reports
        #[arg(long)]
        quiet: bool,
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
        Command::Check { source, target, contract, json, records, csv, all, threads, quiet } => {
            if let Some(n) = threads {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(n.max(1))
                    .build_global()
                    .map_err(|e| e.to_string())?;
            }
            let text = std::fs::read_to_string(&contract).map_err(|e| format!("{}: {e}", contract.display()))?;
            let config = Config::from_json(&text)?;

            let loading = Instant::now();
            let (s, t) = rayon::join(|| Dataset::from_csv(&source), || Dataset::from_csv(&target));
            let (s, t) = (s?, t?);
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
            if !quiet {
                report::print_summary(&mut std::io::stdout().lock(), &outcome, &meta, load_ms)
                    .map_err(|e| e.to_string())?;
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
