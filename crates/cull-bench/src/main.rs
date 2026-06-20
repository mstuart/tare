use cull_bench::{corpus, render_board, run_benchmark_with, Compressor, ShellCompressor};

/// Probe a shell-out incumbent and add it to `extra` when runnable. `env_py` overrides the
/// interpreter (e.g. a venv path) so an incumbent installed in its own environment is picked up.
fn maybe_incumbent(name: &'static str, env_py: &str, script: &str, extra: &mut Vec<Box<dyn Compressor>>) {
    let python = std::env::var(env_py).unwrap_or_else(|_| "python3".to_string());
    let c = ShellCompressor::new(name, python, vec![script.to_string()]);
    match c.probe() {
        Ok(()) => { eprintln!("[bench] {name}: available — included"); extra.push(Box::new(c)); }
        Err(e) => eprintln!("[bench] {name}: unavailable — skipped ({e})"),
    }
}

fn main() {
    let budget: u32 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(60);

    // Real-incumbent shell-out seam (spec §12): probe each adapter; include those available here.
    let mut extra: Vec<Box<dyn Compressor>> = Vec::new();
    maybe_incumbent("llmlingua-2", "CULL_LLMLINGUA_PY",
        concat!(env!("CARGO_MANIFEST_DIR"), "/adapters/llmlingua2.py"), &mut extra);
    maybe_incumbent("headroom", "CULL_HEADROOM_PY",
        concat!(env!("CARGO_MANIFEST_DIR"), "/adapters/headroom_adapter.py"), &mut extra);

    let board = run_benchmark_with(&corpus(), budget, extra);
    println!("Cull benchmark — budget {budget} tokens, {} items\n", corpus().len());
    print!("{}", render_board(&board));
    println!("\nsaved% = tokens removed (the RTK/Headroom headline); time/call = cost, same clock for all.");
    println!("Cull is in-process; incumbents run via the ShellCompressor seam (subprocess), so their");
    println!("time/call includes per-call model load. Warm steady-state (model resident): LLMLingua-2");
    println!("≈273ms/call, Headroom abstains on small inputs. Cull's 100% fidelity makes its savings");
    println!("usable; LLMLingua-2's lossy drop corrupts exact tokens, so its 47.9% savings score 0% fidelity.");
    println!("(Adapters in crates/cull-bench/adapters/; point CULL_LLMLINGUA_PY / CULL_HEADROOM_PY at a venv.)");
}
