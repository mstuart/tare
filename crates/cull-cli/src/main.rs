use std::io::Read;
use clap::{Parser, Subcommand};
use cull_cli::run_compress;

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
    },
}

fn main() {
    let cli = Cli::parse();
    match cli.cmd {
        Command::Compress { task, report } => {
            let mut input = String::new();
            if std::io::stdin().read_to_string(&mut input).is_err() {
                eprintln!("error: failed to read stdin");
                std::process::exit(1);
            }
            match run_compress(&input, &task) {
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
    }
}
