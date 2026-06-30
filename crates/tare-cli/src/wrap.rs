//! `tare wrap` / `tare unwrap` — start tare-proxy and launch a coding agent through it.

use std::net::TcpStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

// ─── Agent registry ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum AgentKind {
    /// We start the proxy and then exec this binary.
    Cli,
    /// GUI / VS Code extension / library — print setup instructions only.
    Manual,
}

#[derive(Debug, Clone)]
pub struct Agent {
    pub name: &'static str,
    pub kind: AgentKind,
    /// Binary to invoke (same as name for Cli agents; "" for Manual agents).
    pub binary: &'static str,
    /// Tool-specific config hint for the user (Manual agents only).
    pub hint: &'static str,
}

pub static REGISTRY: &[Agent] = &[
    // ── CLI agents (binary name == agent name) ──────────────────────────────
    Agent {
        name: "claude",
        kind: AgentKind::Cli,
        binary: "claude",
        hint: "",
    },
    Agent {
        name: "codex",
        kind: AgentKind::Cli,
        binary: "codex",
        hint: "",
    },
    Agent {
        name: "aider",
        kind: AgentKind::Cli,
        binary: "aider",
        hint: "",
    },
    Agent {
        name: "goose",
        kind: AgentKind::Cli,
        binary: "goose",
        hint: "",
    },
    Agent {
        name: "openhands",
        kind: AgentKind::Cli,
        binary: "openhands",
        hint: "",
    },
    Agent {
        name: "opencode",
        kind: AgentKind::Cli,
        binary: "opencode",
        hint: "",
    },
    Agent {
        name: "openclaw",
        kind: AgentKind::Cli,
        binary: "openclaw",
        hint: "",
    },
    Agent {
        name: "vibe",
        kind: AgentKind::Cli,
        binary: "vibe",
        hint: "",
    },
    // ── Manual agents (GUI / extension / library) ───────────────────────────
    Agent {
        name: "cursor",
        kind: AgentKind::Manual,
        binary: "",
        hint: "Settings → override base URL",
    },
    Agent {
        name: "cline",
        kind: AgentKind::Manual,
        binary: "",
        hint: "the extension's base-URL setting",
    },
    Agent {
        name: "continue",
        kind: AgentKind::Manual,
        binary: "",
        hint: "the extension's baseURL setting",
    },
    Agent {
        name: "cortex",
        kind: AgentKind::Manual,
        binary: "",
        hint: "the library/proxy base URL option",
    },
];

/// Look up an agent by name from the registry.
pub fn find_agent(name: &str) -> Option<&'static Agent> {
    REGISTRY.iter().find(|a| a.name == name)
}

fn supported_names() -> String {
    REGISTRY
        .iter()
        .map(|a| a.name)
        .collect::<Vec<_>>()
        .join(", ")
}

// ─── Wrap plan (pure, side-effect-free) ──────────────────────────────────────

/// A pure description of what `tare wrap` would execute for a CLI agent.
pub struct WrapPlan {
    /// Path (or name) of the tare-proxy binary.
    pub proxy_bin: String,
    /// Port the proxy will listen on.
    pub proxy_port: u16,
    /// Derived base URL, e.g. `http://127.0.0.1:8787`.
    #[allow(dead_code)]
    pub base_url: String,
    /// The three env-var pairs forwarded to the agent:
    /// ANTHROPIC_BASE_URL, OPENAI_BASE_URL, OPENAI_API_BASE.
    pub env_exports: Vec<(String, String)>,
    /// Agent binary name.
    pub agent_bin: String,
    /// Extra args forwarded verbatim to the agent.
    pub agent_args: Vec<String>,
}

impl WrapPlan {
    /// Returns a human-readable description of what would be executed (no I/O, fully testable).
    pub fn describe(&self) -> String {
        let mut lines = Vec::new();
        lines.push(format!(
            "proxy:  TARE_PORT={} {}",
            self.proxy_port, self.proxy_bin
        ));
        for (k, v) in &self.env_exports {
            lines.push(format!("export {k}={v}"));
        }
        let args_str = self.agent_args.join(" ");
        if args_str.is_empty() {
            lines.push(format!("agent:  {}", self.agent_bin));
        } else {
            lines.push(format!("agent:  {} {}", self.agent_bin, args_str));
        }
        lines.join("\n")
    }
}

/// Build a [`WrapPlan`] for `agent` on `port`, forwarding `args` to the binary.
pub fn make_wrap_plan(agent: &Agent, port: u16, args: &[String]) -> WrapPlan {
    let proxy_bin = find_proxy_bin().to_string_lossy().into_owned();
    let base_url = format!("http://127.0.0.1:{port}");
    let env_exports = vec![
        ("ANTHROPIC_BASE_URL".into(), base_url.clone()),
        ("OPENAI_BASE_URL".into(), base_url.clone()),
        ("OPENAI_API_BASE".into(), base_url.clone()),
    ];
    WrapPlan {
        proxy_bin,
        proxy_port: port,
        base_url,
        env_exports,
        agent_bin: agent.binary.to_string(),
        agent_args: args.to_vec(),
    }
}

fn find_proxy_bin() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("tare-proxy");
            if candidate.exists() {
                return candidate;
            }
        }
    }
    PathBuf::from("tare-proxy")
}

// ─── Port readiness ───────────────────────────────────────────────────────────

fn wait_for_port(port: u16, timeout: Duration) -> bool {
    let addr = format!("127.0.0.1:{port}");
    let deadline = Instant::now() + timeout;
    loop {
        if TcpStream::connect(&addr).is_ok() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

// ─── wrap ─────────────────────────────────────────────────────────────────────

/// Run `tare wrap`. Returns an exit code (0 = success).
pub fn run_wrap(agent_name: &str, port: u16, print_only: bool, args: &[String]) -> i32 {
    let agent = match find_agent(agent_name) {
        Some(a) => a,
        None => {
            eprintln!("error: unknown agent {agent_name:?}");
            eprintln!("supported: {}", supported_names());
            return 1;
        }
    };

    match agent.kind {
        AgentKind::Manual => {
            let base_url = format!("http://127.0.0.1:{port}");
            println!("To use {} with tare:", agent.name);
            println!(
                "  1. Start the proxy:  tare-proxy  \
(or: TARE_PORT={port} tare-proxy for a custom port)"
            );
            println!("  2. Point {}'s base URL at: {base_url}", agent.name);
            println!("     Hint: {}", agent.hint);
            0
        }
        AgentKind::Cli => {
            let plan = make_wrap_plan(agent, port, args);
            if print_only {
                println!("{}", plan.describe());
                return 0;
            }
            run_cli_agent(plan)
        }
    }
}

fn run_cli_agent(plan: WrapPlan) -> i32 {
    use std::process::Stdio;

    // Start proxy in the background.
    let mut proxy = match std::process::Command::new(&plan.proxy_bin)
        .env("TARE_PORT", plan.proxy_port.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "error: failed to start tare-proxy ({:?}): {e}",
                plan.proxy_bin
            );
            return 1;
        }
    };

    // Wait until the proxy accepts connections (up to 5 s).
    if !wait_for_port(plan.proxy_port, Duration::from_secs(5)) {
        eprintln!(
            "error: tare-proxy did not become ready on port {} within 5s",
            plan.proxy_port
        );
        let _ = proxy.kill();
        return 1;
    }

    // Spawn the agent with the three base-URL env vars, inheriting stdio.
    let spawn_result = std::process::Command::new(&plan.agent_bin)
        .args(&plan.agent_args)
        .envs(plan.env_exports.clone())
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn();

    let exit_code = match spawn_result {
        Ok(mut child) => match child.wait() {
            Ok(status) => status.code().unwrap_or(1),
            Err(e) => {
                eprintln!("error: waiting for agent: {e}");
                1
            }
        },
        Err(e) => {
            eprintln!("error: failed to spawn agent {:?}: {e}", plan.agent_bin);
            1
        }
    };

    // Always kill the proxy, even if the agent failed to spawn.
    let _ = proxy.kill();
    exit_code
}

// ─── unwrap ───────────────────────────────────────────────────────────────────

/// Run `tare unwrap`. Returns an exit code (0 = success).
pub fn run_unwrap(agent_name: &str) -> i32 {
    if find_agent(agent_name).is_none() {
        eprintln!("error: unknown agent {agent_name:?}");
        eprintln!("supported: {}", supported_names());
        return 1;
    }
    println!(
        "Wrapping is ENV-based and ephemeral: `tare wrap` sets ANTHROPIC_BASE_URL, \
OPENAI_BASE_URL, and OPENAI_API_BASE only for the duration of that invocation — \
there is no persistent global state to remove.\n\
If you configured a base-URL override directly in {agent_name}'s settings, \
remove it there."
    );
    0
}

// ─── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_known_agents_resolve() {
        for name in &[
            "claude",
            "codex",
            "aider",
            "goose",
            "openhands",
            "opencode",
            "openclaw",
            "vibe",
            "cursor",
            "cline",
            "continue",
            "cortex",
        ] {
            assert!(
                find_agent(name).is_some(),
                "agent {name:?} not found in registry"
            );
        }
    }

    #[test]
    fn unknown_agent_not_found() {
        assert!(find_agent("notanagent").is_none());
        assert!(find_agent("").is_none());
        assert!(find_agent("gpt4").is_none());
    }

    #[test]
    fn cli_agents_binary_equals_name() {
        for agent in REGISTRY.iter().filter(|a| a.kind == AgentKind::Cli) {
            assert_eq!(
                agent.binary, agent.name,
                "CLI agent {:?}: binary should equal name",
                agent.name
            );
        }
    }

    #[test]
    fn wrap_plan_base_url() {
        let agent = find_agent("claude").unwrap();
        let plan = make_wrap_plan(agent, 8787, &[]);
        assert_eq!(plan.base_url, "http://127.0.0.1:8787");
    }

    #[test]
    fn wrap_plan_all_three_env_vars_present_and_correct() {
        let agent = find_agent("claude").unwrap();
        let plan = make_wrap_plan(agent, 8787, &[]);
        let keys: Vec<&str> = plan.env_exports.iter().map(|(k, _)| k.as_str()).collect();
        assert!(
            keys.contains(&"ANTHROPIC_BASE_URL"),
            "ANTHROPIC_BASE_URL missing"
        );
        assert!(keys.contains(&"OPENAI_BASE_URL"), "OPENAI_BASE_URL missing");
        assert!(keys.contains(&"OPENAI_API_BASE"), "OPENAI_API_BASE missing");
        for (k, v) in &plan.env_exports {
            assert_eq!(v, "http://127.0.0.1:8787", "env var {k} has wrong value");
        }
    }

    #[test]
    fn wrap_plan_agent_bin_is_agent_name() {
        let agent = find_agent("claude").unwrap();
        let plan = make_wrap_plan(agent, 8787, &[]);
        assert_eq!(plan.agent_bin, "claude");
    }

    #[test]
    fn wrap_claude_describe_contains_required_fields() {
        let agent = find_agent("claude").unwrap();
        let plan = make_wrap_plan(agent, 8787, &[]);
        let desc = plan.describe();
        assert!(
            desc.contains("http://127.0.0.1:8787"),
            "base URL missing from describe output:\n{desc}"
        );
        assert!(
            desc.contains("ANTHROPIC_BASE_URL=http://127.0.0.1:8787"),
            "ANTHROPIC_BASE_URL missing from describe output:\n{desc}"
        );
        assert!(
            desc.contains("claude"),
            "agent name 'claude' missing from describe output:\n{desc}"
        );
    }

    #[test]
    fn wrap_plan_custom_port_and_args() {
        let agent = find_agent("aider").unwrap();
        let args = vec!["--model".to_string(), "gpt-4".to_string()];
        let plan = make_wrap_plan(agent, 9000, &args);
        assert_eq!(plan.proxy_port, 9000);
        assert_eq!(plan.base_url, "http://127.0.0.1:9000");
        assert_eq!(plan.agent_bin, "aider");
        assert_eq!(plan.agent_args, args);
        let desc = plan.describe();
        assert!(
            desc.contains("9000"),
            "port 9000 missing from describe:\n{desc}"
        );
        assert!(
            desc.contains("--model gpt-4"),
            "args missing from describe:\n{desc}"
        );
    }
}
