mod admin;
mod dashboard;
mod doctor;
mod learn;
mod output_savings;
mod perf;
mod update;
mod wrap;

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
        /// File path, used for language detection (.rs/.py/.js/.ts/.tsx/.go/.java/.c/.h/.cpp/.cc/.cxx/.hpp/.hh/.hxx/.pl/.pm).
        #[arg(long)]
        path: String,
    },
    /// Opt-in LOSSY HTML compaction for LLM context (read from stdin): strip <script>, <style>,
    /// and <svg> blocks; HTML comments; and noisy presentational attributes (style/class/data-*/on*).
    /// Collapses whitespace and drops empty lines. Keeps text content and semantic tag structure.
    /// Passthrough if the input is not HTML-ish or the result would not be smaller.
    CompactHtml,
    /// Opt-in LOSSY CSV/TSV row compaction for LLM context (read from stdin): detect the delimiter
    /// (comma/tab), always keep the header and the first/last `boundary` data rows plus anomalous
    /// rows (wrong column count or alert keywords), drop the uniform bulk with an explicit marker.
    CompactCsv {
        /// How many head and tail data rows to always keep (schema + recency). Default: 3.
        #[arg(long, default_value_t = 3)]
        boundary: usize,
        /// Cap on total kept data rows; 0 = uncapped. Mandatory rows (boundary + anomalies) are
        /// always kept regardless. Default: 0 (uncapped).
        #[arg(long, default_value_t = 0)]
        max_rows: usize,
    },
    /// Opt-in LOSSY image stripping (read from stdin): replace base64-encoded inline images with
    /// compact `[tare-image id=… fmt=… ~NKB]` markers. Passes through unchanged if no inline images
    /// are detected. The MCP tool is the reversible path; this CLI direction is one-way.
    DerefImages,
    /// Health check: engine self-test, tokenizer sanity, config report, proxy probe, and learned
    /// profile status. Exits non-zero if any ✗ check fails.
    Doctor,
    /// Measure compression savings and speed. Use --input to supply a file or directory; omit it
    /// to run on a built-in representative sample corpus.
    Perf {
        /// File or directory to benchmark. Omit to use the built-in sample corpus.
        #[arg(long)]
        input: Option<std::path::PathBuf>,
        /// Use the built-in sample corpus (same as omitting --input).
        #[arg(long, default_value_t = false)]
        sample: bool,
    },
    /// Derive and persist a compression profile by analysing files under DIR.
    Learn {
        /// Directory to read source/data files from.
        #[arg(long)]
        from: std::path::PathBuf,
    },
    /// Live savings dashboard: poll the proxy's /admin/stats and render a panel.
    Dashboard {
        /// Proxy port to poll (defaults to $TARE_PORT or 8787).
        #[arg(long)]
        port: Option<u16>,
        /// Print a single snapshot and exit (for scripting).
        #[arg(long, default_value_t = false)]
        once: bool,
        /// Refresh interval in milliseconds.
        #[arg(long, default_value_t = 1000)]
        interval_ms: u64,
    },
    /// Estimate output-token reduction from the proxy's A/B holdout (needs TARE_OUTPUT_HOLDOUT > 0).
    OutputSavings {
        /// Proxy port to poll (defaults to $TARE_PORT or 8787).
        #[arg(long)]
        port: Option<u16>,
    },
    /// Self-upgrade to the latest GitHub release. With --check, only report; make no changes.
    Update {
        /// Only check the latest version and report; do not modify anything.
        #[arg(long, default_value_t = false)]
        check: bool,
    },
    /// Start the tare proxy and launch a coding agent through it, forwarding
    /// ANTHROPIC_BASE_URL / OPENAI_BASE_URL / OPENAI_API_BASE to the agent process.
    /// For GUI/extension agents (cursor, cline, continue, cortex) print setup instructions instead.
    Wrap {
        /// Agent to wrap: claude, codex, aider, goose, openhands, opencode, openclaw, vibe,
        /// cursor, cline, continue, cortex.
        agent: String,
        /// Proxy port (defaults to $TARE_PORT or 8787).
        #[arg(long)]
        port: Option<u16>,
        /// Dry-run: print what would happen and exit without starting anything.
        #[arg(long, default_value_t = false)]
        print: bool,
        /// Extra arguments forwarded verbatim to the agent binary (after --).
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Print instructions for removing the tare proxy override from an agent.
    /// (Wrapping is ENV-based and ephemeral — there is no persistent state to remove.)
    Unwrap {
        /// Agent name (same set as `wrap`).
        agent: String,
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
        Command::CompactHtml => {
            let mut input = String::new();
            if std::io::stdin().read_to_string(&mut input).is_err() {
                eprintln!("error: failed to read stdin");
                std::process::exit(1);
            }
            let in_bytes = input.len();
            let out = tare_core::html_compact::compact(&input).unwrap_or(input);
            let out_bytes = out.len();
            let ratio = if in_bytes > 0 {
                out_bytes as f64 / in_bytes as f64
            } else {
                1.0
            };
            println!("{out}");
            eprintln!("[tare] in={in_bytes}B out={out_bytes}B ratio={ratio:.3}");
        }
        Command::CompactCsv { boundary, max_rows } => {
            let mut input = String::new();
            if std::io::stdin().read_to_string(&mut input).is_err() {
                eprintln!("error: failed to read stdin");
                std::process::exit(1);
            }
            let in_bytes = input.len();
            let out = tare_core::csv_compact::compact(&input, boundary, max_rows).unwrap_or(input);
            let out_bytes = out.len();
            let ratio = if in_bytes > 0 {
                out_bytes as f64 / in_bytes as f64
            } else {
                1.0
            };
            println!("{out}");
            eprintln!("[tare] in={in_bytes}B out={out_bytes}B ratio={ratio:.3}");
        }
        Command::DerefImages => {
            let mut input = String::new();
            if std::io::stdin().read_to_string(&mut input).is_err() {
                eprintln!("error: failed to read stdin");
                std::process::exit(1);
            }
            let in_bytes = input.len();
            match tare_core::image_deref::deref(&input) {
                Some(d) => {
                    let out_bytes = d.text.len();
                    let ratio = if in_bytes > 0 {
                        out_bytes as f64 / in_bytes as f64
                    } else {
                        1.0
                    };
                    println!("{}", d.text);
                    eprintln!(
                        "[tare] in={in_bytes}B out={out_bytes}B images={} ratio={ratio:.3}",
                        d.images.len()
                    );
                }
                None => {
                    println!("{input}");
                }
            }
        }
        Command::Doctor => {
            let result = doctor::run();
            if !result.ok {
                std::process::exit(1);
            }
        }
        Command::Perf { input, sample: _ } => match input {
            Some(path) => {
                perf::run_path(&path);
            }
            None => {
                perf::run_sample();
            }
        },
        Command::Learn { from } => {
            if let Err(e) = learn::run(&from) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Command::Dashboard {
            port,
            once,
            interval_ms,
        } => {
            dashboard::run(dashboard::DashboardOpts {
                port: resolve_port(port),
                once,
                interval_ms,
            });
        }
        Command::OutputSavings { port } => {
            output_savings::run(output_savings::OutputSavingsOpts {
                port: resolve_port(port),
            });
        }
        Command::Update { check } => {
            update::run(update::UpdateOpts { check });
        }
        Command::Wrap {
            agent,
            port,
            print,
            args,
        } => {
            let code = wrap::run_wrap(&agent, resolve_port(port), print, &args);
            if code != 0 {
                std::process::exit(code);
            }
        }
        Command::Unwrap { agent } => {
            let code = wrap::run_unwrap(&agent);
            if code != 0 {
                std::process::exit(code);
            }
        }
    }
}

/// Resolve the proxy port from the flag, then `$TARE_PORT`, then the 8787 default.
fn resolve_port(opt: Option<u16>) -> u16 {
    opt.or_else(|| std::env::var("TARE_PORT").ok().and_then(|p| p.parse().ok()))
        .unwrap_or(8787)
}
