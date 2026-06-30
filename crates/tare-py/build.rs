fn main() {
    // On macOS, `cargo build` for a cdylib extension module must allow undefined
    // Python symbols (the embedding interpreter resolves them at load time).
    // PyO3 0.20+ no longer emits this automatically; maturin adds it for you,
    // but raw cargo needs it explicitly.
    #[cfg(target_os = "macos")]
    {
        println!("cargo:rustc-link-arg=-undefined");
        println!("cargo:rustc-link-arg=dynamic_lookup");
    }
}
