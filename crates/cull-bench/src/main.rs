use cull_bench::{corpus, render_board, run_benchmark_with, Compressor, ShellCompressor};

fn main() {
    let budget: u32 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(60);

    // Real-incumbent shell-out seam (spec §12): probe each adapter; include the ones available here.
    // CULL_PY overrides the interpreter (e.g. a venv path) so an installed incumbent is picked up.
    let mut extra: Vec<Box<dyn Compressor>> = Vec::new();
    let python = std::env::var("CULL_PY").unwrap_or_else(|_| "python3".to_string());
    let llmlingua = ShellCompressor::new(
        "llmlingua-2", python,
        vec![concat!(env!("CARGO_MANIFEST_DIR"), "/adapters/llmlingua2.py").to_string()],
    );
    match llmlingua.probe() {
        Ok(()) => { eprintln!("[bench] llmlingua-2: available — included"); extra.push(Box::new(llmlingua)); }
        Err(e) => eprintln!("[bench] llmlingua-2: unavailable — skipped ({e})"),
    }

    let board = run_benchmark_with(&corpus(), budget, extra);
    println!("Cull benchmark — budget {budget} tokens, {} items\n", corpus().len());
    print!("{}", render_board(&board));
    println!("\nLower ratio = more compressed; higher fidelity = task-relevant content preserved.");
    println!("(Incumbents run via the ShellCompressor seam — adapters in crates/cull-bench/adapters/;");
    println!(" install one, e.g. `pip install llmlingua`, and it appears in the board automatically.)");
}
