//! `tare update` — self-upgrade. Compares the running version against the latest GitHub release
//! and (without `--check`) re-runs the appropriate installer (npm or curl) detected from the binary
//! path. No new crate deps: the GitHub API and the installer are reached by shelling out to `curl`.

use std::cmp::Ordering;
use std::process::Command;

pub struct UpdateOpts {
    pub check: bool,
}

const REPO: &str = "mstuart/tare";
const CURRENT: &str = env!("CARGO_PKG_VERSION");

pub fn run(opts: UpdateOpts) {
    let latest = match latest_release_tag() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("tare update: could not fetch the latest release: {e}");
            std::process::exit(1);
        }
    };
    let latest_ver = latest.trim_start_matches('v');

    println!("current: v{CURRENT}");
    println!("latest : v{latest_ver}");

    let newer = cmp_semver(CURRENT, latest_ver) == Ordering::Less;
    if newer {
        println!("→ a newer version is available.");
    } else {
        println!("→ already up to date.");
    }

    // `--check` never mutates; and there's nothing to do if we're already current.
    if opts.check || !newer {
        return;
    }

    match detect_install_method() {
        InstallMethod::Npm => {
            println!("Upgrading via npm: npm install -g tare-ai@latest");
            report(
                Command::new("npm")
                    .args(["install", "-g", "tare-ai@latest"])
                    .status(),
            );
        }
        InstallMethod::Curl => {
            println!("Upgrading via the install script (curl | sh)…");
            report(
                Command::new("sh")
                    .arg("-c")
                    .arg(format!(
                        "curl -fsSL https://raw.githubusercontent.com/{REPO}/main/install.sh | sh"
                    ))
                    .status(),
            );
        }
    }
}

fn latest_release_tag() -> Result<String, String> {
    let out = Command::new("curl")
        .args([
            "-fsSL",
            "-H",
            "Accept: application/vnd.github+json",
            &format!("https://api.github.com/repos/{REPO}/releases/latest"),
        ])
        .output()
        .map_err(|e| format!("could not run curl ({e}); is it installed?"))?;
    if !out.status.success() {
        return Err(format!("curl exited with {}", out.status));
    }
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("bad GitHub response: {e}"))?;
    v.get("tag_name")
        .and_then(|t| t.as_str())
        .map(str::to_string)
        .ok_or_else(|| "no tag_name in the GitHub release response".into())
}

enum InstallMethod {
    Npm,
    Curl,
}

fn detect_install_method() -> InstallMethod {
    if let Ok(exe) = std::env::current_exe() {
        let p = exe.to_string_lossy();
        if p.contains("node_modules") || p.contains("/npm/") || p.contains("/.npm/") {
            return InstallMethod::Npm;
        }
    }
    InstallMethod::Curl
}

fn report(status: std::io::Result<std::process::ExitStatus>) {
    match status {
        Ok(s) if s.success() => println!("✓ upgrade complete."),
        Ok(s) => {
            eprintln!("upgrade command exited with {s}");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("upgrade failed to start: {e}");
            std::process::exit(1);
        }
    }
}

/// Compare two dotted numeric versions (e.g. "0.1.0"); non-numeric parts count as 0.
fn cmp_semver(a: &str, b: &str) -> Ordering {
    parse_ver(a).cmp(&parse_ver(b))
}

fn parse_ver(v: &str) -> (u64, u64, u64) {
    let mut it = v.trim_start_matches('v').split('.').map(|x| {
        x.chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse::<u64>()
            .unwrap_or(0)
    });
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_compare() {
        assert_eq!(cmp_semver("0.1.0", "0.1.0"), Ordering::Equal);
        assert_eq!(cmp_semver("0.1.0", "0.1.1"), Ordering::Less);
        assert_eq!(cmp_semver("0.2.0", "0.1.9"), Ordering::Greater);
        assert_eq!(cmp_semver("1.0.0", "0.9.9"), Ordering::Greater);
        assert_eq!(cmp_semver("v0.1.0", "0.1.0"), Ordering::Equal);
        assert_eq!(cmp_semver("0.1.0", "0.1"), Ordering::Equal);
    }
}
