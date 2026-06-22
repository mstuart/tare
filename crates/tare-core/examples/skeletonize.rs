//! Skeletonize a source file: keep signatures, types, and imports; drop function bodies.
//!
//! Run with:
//!     cargo run -p tare-core --example skeletonize -- path/to/file.rs
//!
//! Supports rust/python/js/ts/go (by extension). Prints the original unchanged if the language is
//! unknown or there is nothing worth eliding.

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "crates/tare-core/examples/skeletonize.rs".to_string());
    let src = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {path}: {e}");
            std::process::exit(1);
        }
    };
    match tare_core::code_skeleton::skeletonize(&src, &path) {
        Some(skeleton) => {
            eprintln!("[skeletonized: {} -> {} bytes]", src.len(), skeleton.len());
            print!("{skeleton}");
        }
        None => {
            eprintln!("[unchanged: unknown language or nothing to elide]");
            print!("{src}");
        }
    }
}
