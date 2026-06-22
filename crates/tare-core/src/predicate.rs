use crate::task::TaskSignal;
use serde_json::Value;

/// Rule-based predicate pushdown (spec §7 D1): narrow an over-broad search tool call using the
/// task, BEFORE execution. Conservative — only narrows repo-wide (`.`/missing scope) searches to a
/// task-relevant path symbol; everything else is returned unchanged. Intended for a tool-execution
/// hook (the model-boundary proxy cannot rewrite tool calls).
pub fn narrow_tool_call(name: &str, input: &Value, task: &TaskSignal) -> Value {
    let mut out = input.clone();
    let l = name.to_ascii_lowercase();
    let is_search = ["grep", "rg", "ripgrep", "find", "search"]
        .iter()
        .any(|s| l.contains(s));
    if !is_search {
        return out;
    }

    let scope = out
        .get("path")
        .or_else(|| out.get("dir"))
        .or_else(|| out.get("scope"))
        .and_then(Value::as_str);
    let repo_wide = matches!(scope, None | Some(".") | Some("./"));
    if !repo_wide {
        return out;
    }

    // pick the most specific path-like task symbol (has a '/')
    if let Some(p) = task
        .symbols
        .iter()
        .filter(|s| s.contains('/'))
        .max_by_key(|s| s.len())
    {
        if let Some(obj) = out.as_object_mut() {
            obj.insert("path".into(), Value::String(p.clone()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::TaskSignal;
    use serde_json::json;

    #[test]
    fn narrows_repo_wide_grep_to_task_path() {
        let task = TaskSignal::from_text("fix the auth bug in src/auth");
        let input = json!({"query": "TODO", "path": "."});
        let out = narrow_tool_call("grep", &input, &task);
        assert_eq!(out["path"], "src/auth"); // narrowed
        assert_eq!(out["query"], "TODO"); // other args preserved
    }

    #[test]
    fn leaves_already_scoped_or_non_search_calls_unchanged() {
        let task = TaskSignal::from_text("work in src/auth");
        let scoped = json!({"query":"x","path":"src/db"});
        assert_eq!(narrow_tool_call("grep", &scoped, &task), scoped); // already scoped
        let read = json!({"path":"."});
        assert_eq!(narrow_tool_call("read_file", &read, &task), read); // not a search
        let no_task = json!({"query":"x","path":"."});
        assert_eq!(
            narrow_tool_call("grep", &no_task, &TaskSignal::empty()),
            no_task
        ); // no task path
    }
}
