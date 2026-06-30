// pyo3 0.22: new_err's generic IntoPyErrArguments triggers useless_conversion false positive
#![allow(clippy::useless_conversion)]
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

/// Run the full tare compression pipeline on a JSON context array and an optional task string.
/// Raises `ValueError` on parse or pipeline errors.
#[pyfunction]
#[pyo3(signature = (blocks_json, task = ""))]
fn compress(blocks_json: &str, task: &str) -> PyResult<String> {
    tare_cli::run_compress(blocks_json, task)
        .map(|out| out.compressed)
        .map_err(PyValueError::new_err)
}

/// Skeletonize source text for the file at `path`: replaces function bodies with
/// `… N lines elided` markers. Returns the input unchanged when the language is
/// unknown or the result would not be smaller.
#[pyfunction]
fn skeletonize(text: &str, path: &str) -> String {
    tare_core::code_skeleton::skeletonize(text, path).unwrap_or_else(|| text.to_owned())
}

/// Lossy compaction for tabular / log text. Returns the input unchanged when no
/// compaction is possible.
#[pyfunction]
#[pyo3(signature = (text, boundary = 3, task = "", max_field = 0, max_rows = 0))]
fn compact_lossy(
    text: &str,
    boundary: usize,
    task: &str,
    max_field: usize,
    max_rows: usize,
) -> String {
    let task_opt = if task.is_empty() { None } else { Some(task) };
    tare_core::lossy_compact::compact_opts(text, boundary, task_opt, max_field, max_rows)
        .unwrap_or_else(|| text.to_owned())
}

/// Slim a JSON Schema document, dropping verbose boilerplate. Returns the input
/// unchanged when not applicable.
#[pyfunction]
fn slim_schema(text: &str) -> String {
    tare_core::schema_slim::slim(text).unwrap_or_else(|| text.to_owned())
}

/// Telegraphic compaction: collapses whitespace, removes filler words, and
/// shortens prose to a dense telegraphic form. Returns the input unchanged when
/// not applicable.
#[pyfunction]
fn telegraphic(text: &str) -> String {
    tare_core::telegraphic::compact(text).unwrap_or_else(|| text.to_owned())
}

/// Strip HTML to readable text, removing tags, scripts, and style blocks. Returns
/// the input unchanged when not applicable.
#[pyfunction]
fn compact_html(text: &str) -> String {
    tare_core::html_compact::compact(text).unwrap_or_else(|| text.to_owned())
}

/// Compact a CSV by keeping only the boundary rows at the head/tail plus any
/// query-relevant rows. Returns the input unchanged when not applicable.
#[pyfunction]
#[pyo3(signature = (text, boundary = 3, max_rows = 0))]
fn compact_csv(text: &str, boundary: usize, max_rows: usize) -> String {
    tare_core::csv_compact::compact(text, boundary, max_rows).unwrap_or_else(|| text.to_owned())
}

/// Replace inline base64 data-URI images in `text` with compact `[img:XXXX]`
/// markers and return the cleaned text. Returns the input unchanged when no
/// images are found.
#[pyfunction]
fn deref_images(text: &str) -> String {
    tare_core::image_deref::deref(text)
        .map(|d| d.text)
        .unwrap_or_else(|| text.to_owned())
}

/// Crush a JSON value into tare's compact wire format. Returns `None` when the
/// input is not applicable (not valid JSON or already minimal).
#[pyfunction]
fn crush(text: &str) -> Option<String> {
    tare_core::json_crush::crush(text)
}

/// Expand a previously crushed JSON string back to a pretty-printed JSON string.
/// Returns `None` when the input is not a valid crushed payload.
#[pyfunction]
fn expand(text: &str) -> Option<String> {
    tare_core::json_crush::expand(text).map(|v| v.to_string())
}

#[pymodule]
fn _tare(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(compress, m)?)?;
    m.add_function(wrap_pyfunction!(skeletonize, m)?)?;
    m.add_function(wrap_pyfunction!(compact_lossy, m)?)?;
    m.add_function(wrap_pyfunction!(slim_schema, m)?)?;
    m.add_function(wrap_pyfunction!(telegraphic, m)?)?;
    m.add_function(wrap_pyfunction!(compact_html, m)?)?;
    m.add_function(wrap_pyfunction!(compact_csv, m)?)?;
    m.add_function(wrap_pyfunction!(deref_images, m)?)?;
    m.add_function(wrap_pyfunction!(crush, m)?)?;
    m.add_function(wrap_pyfunction!(expand, m)?)?;
    Ok(())
}
