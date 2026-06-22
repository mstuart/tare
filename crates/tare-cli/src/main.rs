use clap::{Parser, Subcommand};
use std::io::Read;
use tare_cli::run_compress_with_budget;

#[derive(Parser)]
#[command(
    name = "tare",
    about = "Query-aware, cache-correct, lossless context compression"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Compress a JSON context (read from stdin) for a given task.
    Compress {
        /// The current task/query (drives query-conditioned compression).
        #[arg(long, default_value = "")]
        task: String,
        /// Emit the fidelity report as JSON to stderr.
        #[arg(long, default_value_t = false)]
        report: bool,
        /// Optional hard token budget; evict lowest-priority context to fit.
        #[arg(long)]
        budget: Option<u32>,
    },
    /// Opt-in LOSSY transform: strip pure JSON-Schema metadata annotations ($schema, title, $id,
    /// $comment, examples) from tool/function definitions read on stdin. Preserves property names,
    /// types, required, and descriptions. Separate from the lossless `compress` pipeline by design.
    SlimSchema,
    /// Opt-in LOSSY aggressive compaction of a large JSON array (read from stdin): keep the first
    /// and last `boundary` rows + all anomalies (odd shapes, alert keywords), drop the uniform bulk
    /// with an explicit marker. Matches/beats incumbents' row-dropping ratio; lossy by design.
    CompactLossy {
        /// How many head and tail rows to always keep (schema + recency).
        #[arg(long, default_value_t = 3)]
        boundary: usize,
        /// Optional task/query: units relevant to it are always kept (query-aware pruning).
        #[arg(long, default_value = "")]
        task: String,
        /// Aggressive: truncate each kept LINE to this many chars (e.g. the COMMAND column of
        /// `ps aux`) for maximum ratio. 0 = no truncation.
        #[arg(long, default_value_t = 0)]
        max_field: usize,
        /// Aggressive: cap kept LINES to this many (boundary/alert/relevant always kept, the rest
        /// filled by salience). Pairs with --max-field to match a per-command filter. 0 = uncapped.
        #[arg(long, default_value_t = 0)]
        max_rows: usize,
    },
    /// Opt-in LOSSY code skeletonization (read source on stdin): drop function/method bodies, keep
    /// signatures, types, fields, imports, and doc comments — the structure an agent navigates by.
    /// Code reads are ~67-76% of coding-agent tokens; reversible by re-reading. Passthrough if the
    /// language is unknown or nothing is elidable.
    Skeletonize {
        /// File path, used for language detection (.rs/.py/.js/.ts/.tsx/.go).
        #[arg(long)]
        path: String,
    },
}

fn main() {
    let cli = Cli::parse();
    match cli.cmd {
        Command::Compress {
            task,
            report,
            budget,
        } => {
            let mut input = String::new();
            if std::io::stdin().read_to_string(&mut input).is_err() {
                eprintln!("error: failed to read stdin");
                std::process::exit(1);
            }
            match run_compress_with_budget(&input, &task, budget) {
                Ok(out) => {
                    println!("{}", out.compressed);
                    let r = &out.report;
                    eprintln!(
                        "[tare] input={} net={} ratio={:.3} kept={} dropped={} replaced={}",
                        r.input_tokens,
                        r.net_tokens,
                        r.ratio(),
                        r.kept,
                        r.dropped,
                        r.replaced
                    );
                    if report {
                        // simple line-oriented report (JSON serialization of FidelityReport is a later nicety)
                        for (id, reason) in &r.drops {
                            eprintln!("[tare] drop {:?} {:?}", id, reason);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            }
        }
        Command::SlimSchema => {
            let mut input = String::new();
            if std::io::stdin().read_to_string(&mut input).is_err() {
                eprintln!("error: failed to read stdin");
                std::process::exit(1);
            }
            // passthrough if not slimmable (not JSON / no smaller result)
            let out = tare_core::schema_slim::slim(&input).unwrap_or(input);
            println!("{out}");
        }
        Command::CompactLossy {
            boundary,
            task,
            max_field,
            max_rows,
        } => {
            let mut input = String::new();
            if std::io::stdin().read_to_string(&mut input).is_err() {
                eprintln!("error: failed to read stdin");
                std::process::exit(1);
            }
            let t = if task.is_empty() {
                None
            } else {
                Some(task.as_str())
            };
            let out =
                tare_core::lossy_compact::compact_opts(&input, boundary, t, max_field, max_rows)
                    .unwrap_or(input);
            println!("{out}");
        }
        Command::Skeletonize { path } => {
            let mut input = String::new();
            if std::io::stdin().read_to_string(&mut input).is_err() {
                eprintln!("error: failed to read stdin");
                std::process::exit(1);
            }
            let out = tare_core::code_skeleton::skeletonize(&input, &path).unwrap_or(input);
            println!("{out}");
        }
    }
}
