// Exercises run_compress as the binary does, asserting the demoable behavior.
use tare_cli::run_compress;

#[test]
fn cli_core_compresses_superseded_output() {
    let json = r#"[
        {"role":"tool","kind":"tool_output","class":"cargo-test","text":"old failed run"},
        {"role":"tool","kind":"tool_output","class":"cargo-test","text":"new passed run"}
    ]"#;
    let out = run_compress(json, "run tests").unwrap();
    assert!(out.report.net_tokens < out.report.input_tokens);
    assert!(out.compressed.contains("new passed run"));
}
