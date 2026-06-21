use std::io::Read;
use clap::{Parser, Subcommand};
use cull_cli::run_compress_with_budget;

#[derive(Parser)]
#[command(name = "cull", about = "Query-aware, cache-correct, lossless context compression")]
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
    },
}

fn main() {
    let cli = Cli::parse();
    match cli.cmd {
        Command::Compress { task, report, budget } => {
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
                        "[cull] input={} net={} ratio={:.3} kept={} dropped={} replaced={}",
                        r.input_tokens, r.net_tokens, r.ratio(), r.kept, r.dropped, r.replaced
                    );
                    if report {
                        // simple line-oriented report (JSON serialization of FidelityReport is a later nicety)
                        for (id, reason) in &r.drops {
                            eprintln!("[cull] drop {:?} {:?}", id, reason);
                        }
                    }
                }
                Err(e) => { eprintln!("error: {e}"); std::process::exit(1); }
            }
        }
        Command::SlimSchema => {
            let mut input = String::new();
            if std::io::stdin().read_to_string(&mut input).is_err() {
                eprintln!("error: failed to read stdin");
                std::process::exit(1);
            }
            // passthrough if not slimmable (not JSON / no smaller result)
            let out = cull_core::schema_slim::slim(&input).unwrap_or(input);
            println!("{out}");
        }
        Command::CompactLossy { boundary } => {
            let mut input = String::new();
            if std::io::stdin().read_to_string(&mut input).is_err() {
                eprintln!("error: failed to read stdin");
                std::process::exit(1);
            }
            let out = cull_core::lossy_compact::compact(&input, boundary).unwrap_or(input);
            println!("{out}");
        }
    }
}
