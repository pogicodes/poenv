// Is a person at the keyboard, or a script / AI agent? (SPEC §5)
use std::collections::HashMap;
use std::io::IsTerminal;

/// Env vars that agents set in the shells they run. Presence means "not a human", even with a TTY.
/// Sources: community detectors (detect-coding-agent, is-ai-agent, @vercel/detect-agent), checked 2026-11.
pub const AGENT_ENV_VARS: [&str; 9] = [
    "AI_AGENT",
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CODEX_SANDBOX",
    "CODEX_THREAD_ID",
    "CODEX_SANDBOX_NETWORK_DISABLED",
    "CURSOR_AGENT",
    "GEMINI_CLI",
    "CI",
];

/// Agent CLIs, matched as a program name in a parent process's command line. An agent can unset its
/// env vars and fake a TTY (`script`), but the commands it runs still descend from it.
/// Exact, lowercase names: the Claude desktop app ("Claude", "Claude Helper") hosts a person's terminal.
pub const AGENT_PROGRAMS: [&str; 13] = [
    "claude",
    "codex",
    "cursor-agent",
    "gemini",
    "amp",
    "opencode",
    "aider",
    "goose",
    "crush",
    "droid",
    "qwen",
    "kimi",
    "copilot",
];

/// Runtimes that launch agents written in JS (`node …/gemini.js`): their arguments name the agent.
const RUNTIMES: [&str; 3] = ["node", "bun", "deno"];

pub fn agent_marker() -> Option<&'static str> {
    AGENT_ENV_VARS.into_iter().find(|name| std::env::var_os(name).is_some_and(|v| !v.is_empty()))
}

/// The agent a process is, from its executable path (`ps` comm) and, for a JS runtime, its command line.
pub fn agent_program(comm: &str, args: &str) -> Option<&'static str> {
    let name = &comm[comm.rfind('/').map(|i| i + 1).unwrap_or(0)..];
    if let Some(a) = AGENT_PROGRAMS.into_iter().find(|a| *a == name) {
        return Some(a);
    }
    if RUNTIMES.contains(&name) {
        return script_agent(args);
    }
    None
}

/// `/(agent)(?:\.[cm]?js)?(?=\s|$)`, leftmost match first.
fn script_agent(args: &str) -> Option<&'static str> {
    for (i, _) in args.match_indices('/') {
        let rest = &args[i + 1..];
        for agent in AGENT_PROGRAMS {
            let Some(after) = rest.strip_prefix(agent) else { continue };
            let ends = |s: &str| s.is_empty() || s.starts_with(char::is_whitespace);
            if ends(after) {
                return Some(agent);
            }
            for ext in [".js", ".cjs", ".mjs"] {
                if after.strip_prefix(ext).is_some_and(ends) {
                    return Some(agent);
                }
            }
        }
    }
    None
}

/// The first agent among this process's parents, from the process table (macOS, Linux).
pub fn agent_ancestor() -> Option<&'static str> {
    #[cfg(unix)]
    {
        let comms = table("comm")?;
        let args = table("args");
        let mut p = unsafe { libc::getppid() } as u32;
        let mut hops = 0;
        while p > 1 && hops < 64 {
            let Some((ppid, comm)) = comms.get(&p) else { break };
            let a = args.as_ref().and_then(|t| t.get(&p)).map(|(_, a)| a.as_str()).unwrap_or("");
            if let Some(found) = agent_program(comm, a) {
                return Some(found);
            }
            p = *ppid;
            hops += 1;
        }
        None
    }
    #[cfg(not(unix))]
    None
}

/// pid → parent pid and one `ps` column. An absolute path, so a `ps` planted on PATH can't answer.
#[cfg(unix)]
fn table(column: &str) -> Option<HashMap<u32, (u32, String)>> {
    let out = std::process::Command::new("/bin/ps")
        .args(["-A", "-o", &format!("pid=,ppid=,{column}=")])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let mut procs = HashMap::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut parts = line.trim_start().splitn(2, char::is_whitespace);
        let (Some(pid), Some(rest)) = (parts.next(), parts.next()) else { continue };
        let rest = rest.trim_start();
        let mut parts = rest.splitn(2, char::is_whitespace);
        let (Some(ppid), text) = (parts.next(), parts.next().unwrap_or("")) else { continue };
        let (Ok(pid), Ok(ppid)) = (pid.parse::<u32>(), ppid.parse::<u32>()) else { continue };
        procs.insert(pid, (ppid, text.trim_start().trim_end().to_string()));
    }
    Some(procs)
}

pub fn is_interactive_human() -> bool {
    // The process check runs last: it costs a `ps`, and only a TTY without agent markers needs it.
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal() && agent_marker().is_none() && agent_ancestor().is_none()
}

/// Output is left alone only for a person typing in a terminal (stdout may be redirected).
pub fn typing_person() -> bool {
    std::io::stdin().is_terminal() && agent_marker().is_none() && agent_ancestor().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn programs() {
        assert_eq!(agent_program("/usr/local/bin/claude", ""), Some("claude"));
        assert_eq!(agent_program("Claude Helper", ""), None);
        assert_eq!(agent_program("node", "node /opt/lib/gemini.js --x"), Some("gemini"));
        assert_eq!(agent_program("node", "node /opt/server.js"), None);
        assert_eq!(agent_program("bash", "bash /x/claude"), None);
        let app = "/Users/me/Library/Application Support/Claude/claude-code/2.1/claude.app/Contents/MacOS/claude";
        assert_eq!(agent_program(app, ""), Some("claude"));
        assert_eq!(agent_program("codex", ""), Some("codex"));
        assert_eq!(agent_program("node", "node /opt/homebrew/bin/gemini --yolo"), Some("gemini"));
        assert_eq!(agent_program("node", "node /x/cursor-agent.js"), Some("cursor-agent"));
        let helper = "/Applications/Claude.app/Contents/Frameworks/Claude Helper.app/Contents/MacOS/Claude Helper";
        assert_eq!(agent_program(helper, ""), None);
        assert_eq!(agent_program("/Applications/Claude.app/Contents/MacOS/Claude", ""), None);
        assert_eq!(agent_program("/bin/zsh", "/bin/zsh -c cd ~/code/claude"), None);
        assert_eq!(agent_program("node", "node server.js claude"), None);
    }
}
