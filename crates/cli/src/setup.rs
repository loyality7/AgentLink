use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct SetupResult {
    pub target: String,
    pub path: PathBuf,
    pub status: String,
}

fn current_exe_str() -> String {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "agentlink".to_string())
}

fn merge_json_mcp(path: &Path, exe: &str, dry_run: bool) -> Result<String, Box<dyn std::error::Error>> {
    let mut root: Value = if path.exists() {
        let raw = fs::read_to_string(path)?;
        serde_json::from_str(&raw).unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };

    let obj = root
        .as_object_mut()
        .ok_or("expected JSON object in config file")?;
    let servers = obj
        .entry("mcpServers")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or("mcpServers is not an object")?;

    servers.insert(
        "agentlink".to_string(),
        json!({
            "command": exe,
            "args": ["mcp"]
        }),
    );

    if dry_run {
        return Ok("would update".to_string());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_string_pretty(&root)?)?;
    Ok("configured".to_string())
}

fn merge_codex_toml(path: &Path, exe: &str, dry_run: bool) -> Result<String, Box<dyn std::error::Error>> {
    let existing = if path.exists() {
        fs::read_to_string(path)?
    } else {
        String::new()
    };

    let marker_start = "# >>> agentlink >>>";
    let marker_end = "# <<< agentlink <<<";
    let escaped_exe = exe.replace('\\', "\\\\");
    let block = format!(
        "{marker_start}\n[mcp_servers.agentlink]\ncommand = \"{escaped_exe}\"\nargs = [\"mcp\"]\n\n[[hooks.Stop]]\n[[hooks.Stop.hooks]]\ntype = \"command\"\ncommand = \"{escaped_exe} hook-check --format codex-stop\"\n{marker_end}\n"
    );

    let updated = if let (Some(s), Some(e)) = (existing.find(marker_start), existing.find(marker_end)) {
        let end_idx = e + marker_end.len();
        let after = existing[end_idx..].trim_start_matches(['\r', '\n']);
        format!("{}{block}{after}", &existing[..s])
    } else if existing.is_empty() {
        block
    } else {
        format!("{}\n\n{block}", existing.trim_end())
    };

    if dry_run {
        return Ok("would update".to_string());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, updated)?;
    Ok("configured".to_string())
}

fn merge_claude_hooks(path: &Path, exe: &str, dry_run: bool) -> Result<String, Box<dyn std::error::Error>> {
    let mut root: Value = if path.exists() {
        let raw = fs::read_to_string(path)?;
        serde_json::from_str(&raw).unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };

    let hook_cmd = format!("\"{exe}\" hook-check");
    let obj = root
        .as_object_mut()
        .ok_or("expected JSON object in settings.json")?;
    let hooks = obj
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or("hooks is not an object")?;

    for event in ["UserPromptSubmit", "Stop"] {
        let arr = hooks
            .entry(event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or("hook event is not an array")?;
        let already = arr.iter().any(|item| {
            item.to_string().contains("hook-check")
        });
        if !already {
            arr.push(json!({
                "hooks": [{ "type": "command", "command": hook_cmd }]
            }));
        }
    }

    if dry_run {
        return Ok("would update".to_string());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_string_pretty(&root)?)?;
    Ok("configured".to_string())
}

/// Register `agentlink mcp` across Claude Code, Claude Desktop, Codex, and Antigravity.
pub fn run_setup(home_dir: &Path, dry_run: bool) -> Result<Vec<SetupResult>, Box<dyn std::error::Error>> {
    let exe = current_exe_str();
    let mut results = Vec::new();

    // 1. Claude Code (~/.claude.json + ~/.claude/settings.json hooks)
    let claude_code = home_dir.join(".claude.json");
    results.push(SetupResult {
        target: "Claude Code".into(),
        status: merge_json_mcp(&claude_code, &exe, dry_run)?,
        path: claude_code,
    });
    let claude_hooks = home_dir.join(".claude").join("settings.json");
    results.push(SetupResult {
        target: "Claude Hooks".into(),
        status: merge_claude_hooks(&claude_hooks, &exe, dry_run)?,
        path: claude_hooks,
    });

    // 2. Claude Desktop (OS-specific)
    let claude_desktop = if cfg!(target_os = "windows") {
        std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home_dir.join("AppData").join("Roaming"))
            .join("Claude")
            .join("claude_desktop_config.json")
    } else if cfg!(target_os = "macos") {
        home_dir
            .join("Library")
            .join("Application Support")
            .join("Claude")
            .join("claude_desktop_config.json")
    } else {
        home_dir
            .join(".config")
            .join("Claude")
            .join("claude_desktop_config.json")
    };
    results.push(SetupResult {
        target: "Claude Desktop".into(),
        status: merge_json_mcp(&claude_desktop, &exe, dry_run)?,
        path: claude_desktop,
    });

    // 3. Codex (~/.codex/config.toml)
    let codex = home_dir.join(".codex").join("config.toml");
    results.push(SetupResult {
        target: "Codex".into(),
        status: merge_codex_toml(&codex, &exe, dry_run)?,
        path: codex,
    });

    // 4. Antigravity (~/.gemini/antigravity/mcp_config.json)
    let antigravity = home_dir
        .join(".gemini")
        .join("antigravity")
        .join("mcp_config.json");
    results.push(SetupResult {
        target: "Antigravity".into(),
        status: merge_json_mcp(&antigravity, &exe, dry_run)?,
        path: antigravity,
    });

    Ok(results)
}

