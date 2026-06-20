use cull_bench::{corpus, render_board, run_benchmark};

fn main() {
    let budget: u32 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(60);
    let board = run_benchmark(&corpus(), budget);
    println!("Cull benchmark — budget {budget} tokens, {} items\n", corpus().len());
    print!("{}", render_board(&board));
    println!("\nLower ratio = more compressed; higher fidelity = task-relevant content preserved.");
    println!("(Baselines are in-repo; LLMLingua-2/Headroom/Tamp are a shell-out extension behind the same Compressor trait.)");
}
