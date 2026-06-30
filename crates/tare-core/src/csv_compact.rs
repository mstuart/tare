//! Opt-in LOSSY CSV/TSV row compaction for LLM context windows.
//!
//! Detects the delimiter (comma or tab from the header row), always retains the header and the
//! first/last `boundary` data rows, keeps anomalous rows (column-count mismatch or alert
//! keywords), and replaces the dropped uniform bulk with an explicit `… N rows omitted …` marker.
//! Caps total kept data rows at `max_rows` (when > 0), but never drops mandatory rows (boundary +
//! anomalies). Returns `None` if the input is not delimited/tabular or too small to compact.

const ALERTS: &[&str] = &[
    "error",
    "fail",
    "warn",
    "exception",
    "critical",
    "fatal",
    "denied",
    "timeout",
];

/// Compact a CSV or TSV string for LLM context.
///
/// - `boundary`: number of head and tail data rows always retained (schema + recency).
/// - `max_rows`: cap on total kept data rows; 0 = uncapped. Mandatory rows (boundary + anomalous)
///   are always retained regardless of this cap; remaining slots go to the optional kept rows.
///
/// Returns `None` if the input is not tabular, has too few rows, or would not be smaller.
pub fn compact(csv: &str, boundary: usize, max_rows: usize) -> Option<String> {
    let lines: Vec<&str> = csv.lines().collect();
    if lines.len() < 3 {
        return None; // need header + at least 2 data rows
    }
    let delim = detect_delimiter(lines[0])?;

    let header = lines[0];
    let data = &lines[1..];
    let n = data.len();

    // Minimum compactable: enough data rows to make boundary dropping meaningful.
    if n <= 2 * boundary + 2 {
        return None;
    }

    let header_cols = split_row(header, delim).len();
    if header_cols < 2 {
        return None; // single-column input is not tabular enough to compact
    }

    // Build the mandatory keep mask: boundary head/tail rows + anomalous rows.
    let mut mandatory = vec![false; n];
    mandatory[..boundary.min(n)].fill(true);
    mandatory[n.saturating_sub(boundary)..].fill(true);
    for (i, row) in data.iter().enumerate() {
        if split_row(row, delim).len() != header_cols {
            mandatory[i] = true; // shape anomaly
        }
        let lower = row.to_ascii_lowercase();
        if ALERTS.iter().any(|a| lower.contains(a)) {
            mandatory[i] = true; // content anomaly / alert keyword
        }
    }

    // Start with mandatory as the working keep mask, then apply the optional row cap.
    let mut keep = mandatory.clone();
    if max_rows > 0 {
        let kept = keep.iter().filter(|&&k| k).count();
        if kept > max_rows {
            let mand_count = mandatory.iter().filter(|&&m| m).count();
            let optional_budget = max_rows.saturating_sub(mand_count);
            let mut optional_kept = 0usize;
            for i in 0..n {
                if keep[i] && !mandatory[i] {
                    if optional_kept < optional_budget {
                        optional_kept += 1;
                    } else {
                        keep[i] = false;
                    }
                }
            }
        }
    }

    // Render: header + data rows interleaved with omission markers.
    let mut out_lines: Vec<String> = vec![header.to_string()];
    let mut pending_omit = 0usize;
    for (i, row) in data.iter().enumerate() {
        if keep[i] {
            if pending_omit > 0 {
                out_lines.push(format!("… {pending_omit} rows omitted …"));
                pending_omit = 0;
            }
            out_lines.push((*row).to_string());
        } else {
            pending_omit += 1;
        }
    }
    if pending_omit > 0 {
        out_lines.push(format!("… {pending_omit} rows omitted …"));
    }

    let dropped = n - keep.iter().filter(|&&k| k).count();
    if dropped == 0 {
        return None; // nothing elided — passthrough
    }

    let out = out_lines.join("\n");
    if out.len() < csv.len() {
        Some(out)
    } else {
        None
    }
}

/// Detect the field delimiter from the header row: tab when tab-count ≥ comma-count (prefers TSV
/// when tied), comma otherwise. Returns `None` if neither is present.
fn detect_delimiter(header: &str) -> Option<char> {
    let tabs = header.matches('\t').count();
    let commas = header.matches(',').count();
    match (tabs, commas) {
        (0, 0) => None,
        (t, c) if t >= c => Some('\t'),
        _ => Some(','),
    }
}

/// Split a row into fields on `delim`. Naive: no quoted-field handling.
fn split_row(row: &str, delim: char) -> Vec<&str> {
    row.split(delim).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_csv(n: usize) -> String {
        let mut rows = vec!["id,name,status,value".to_string()];
        for i in 0..n {
            rows.push(format!("{i},user{i},ok,{}", i * 10));
        }
        rows.join("\n")
    }

    #[test]
    fn compacts_100_row_csv_to_header_plus_boundary_plus_marker() {
        let csv = make_csv(100);
        let out = compact(&csv, 3, 0).expect("should compact a 100-row CSV");
        // header always first
        assert!(
            out.starts_with("id,name,status,value"),
            "header must be the first line"
        );
        // boundary head rows present
        assert!(
            out.contains("0,user0,ok,0"),
            "first boundary row must be kept"
        );
        assert!(
            out.contains("2,user2,ok,20"),
            "third boundary row must be kept"
        );
        // boundary tail rows present
        assert!(
            out.contains("99,user99,ok,990"),
            "last data row must be kept"
        );
        // omission marker present
        assert!(
            out.contains("rows omitted"),
            "omission marker must be present"
        );
        // uniform middle row dropped
        assert!(
            !out.contains("50,user50"),
            "uniform middle row must be dropped"
        );
        assert!(out.len() < csv.len(), "output must be smaller than input");
    }

    #[test]
    fn header_always_retained() {
        let csv = make_csv(50);
        let out = compact(&csv, 2, 0).expect("should compact");
        assert!(
            out.starts_with("id,name,status,value"),
            "header must always be the first line"
        );
    }

    #[test]
    fn anomalous_rows_are_kept() {
        let mut rows = vec!["id,name,status".to_string()];
        for i in 0..50 {
            rows.push(format!("{i},user{i},ok"));
        }
        // shape anomaly: extra column
        rows[20] = "20,user20,error,extra_col".to_string();
        // content anomaly: alert keyword
        rows[35] = "35,user35,FAILED".to_string();
        let csv = rows.join("\n");
        let out = compact(&csv, 2, 0).expect("should compact");
        assert!(
            out.contains("error,extra_col"),
            "shape-anomaly row must be kept"
        );
        assert!(out.contains("FAILED"), "alert-keyword row must be kept");
    }

    #[test]
    fn too_few_rows_returns_none() {
        // 4 data rows, boundary=3 → 2*3+2=8 > 4 → None
        let csv = "a,b\n1,2\n3,4\n5,6\n7,8".to_string();
        assert!(
            compact(&csv, 3, 0).is_none(),
            "too-small CSV must return None"
        );
    }

    #[test]
    fn non_tabular_returns_none() {
        assert!(
            compact(
                "just some plain text\nno delimiters here\nand more plain lines\nfour of them",
                3,
                0
            )
            .is_none(),
            "non-tabular input must return None"
        );
    }
}
