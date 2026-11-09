use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct McpServer {
    pub id: String,
    pub name: String,
    pub config: String, // Raw JSON or Markdown configuration
    pub is_enabled: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, FromRow)]
pub struct McpServerTool {
    pub server_id: String,
    pub name: String,
    pub config: String, // Markdown tool specification/instructions
}

fn extract_execution_block(content: &str) -> Option<&str> {
    let trimmed = content.trim();
    // 1. Check YAML frontmatter (starts with ---)
    if let Some(rest) = trimmed.strip_prefix("---")
        && let Some(idx) = rest.find("\n---")
    {
        let fm = &rest[..idx];
        if !fm.trim().is_empty() {
            return Some(fm.trim());
        }
    }

    // 2. Check fenced code blocks: ```exec, ```yaml, ```yml, ```json
    for tag in &["```exec", "```yaml", "```yml", "```json"] {
        if let Some(start_idx) = content.find(tag) {
            let after_tag = &content[start_idx + tag.len()..];
            if let Some(end_idx) = after_tag.find("```") {
                let block = after_tag[..end_idx].trim();
                if !block.is_empty() {
                    return Some(block);
                }
            }
        }
    }

    None
}

fn parse_yaml_or_json_value(text: &str) -> serde_json::Value {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return serde_json::Value::Null;
    }

    // Try parsing as JSON first
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed)
        && val.is_object()
    {
        return val;
    }

    // Parse simple YAML lines
    let mut root = serde_json::Map::new();
    let mut current_key: Option<String> = None;
    let mut current_list: Vec<serde_json::Value> = Vec::new();
    let mut current_map: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    let mut is_in_list = false;
    let mut is_in_map = false;

    let flush = |root: &mut serde_json::Map<String, serde_json::Value>,
                 key: &mut Option<String>,
                 list: &mut Vec<serde_json::Value>,
                 map: &mut serde_json::Map<String, serde_json::Value>,
                 in_list: &mut bool,
                 in_map: &mut bool| {
        if let Some(k) = key.take() {
            if *in_list {
                root.insert(k, serde_json::Value::Array(std::mem::take(list)));
                *in_list = false;
            } else if *in_map {
                root.insert(k, serde_json::Value::Object(std::mem::take(map)));
                *in_map = false;
            }
        }
    };

    for line in trimmed.lines() {
        let line_trimmed = line.trim();
        if line_trimmed.is_empty() || line_trimmed.starts_with('#') {
            continue;
        }

        // List item under current key, e.g. - "-y" or - @modelcontextprotocol/server
        if let Some(item_str) = line_trimmed.strip_prefix('-') {
            let item_val = item_str.trim().trim_matches('"').trim_matches('\'');
            if current_key.is_some() {
                is_in_list = true;
                current_list.push(serde_json::Value::String(item_val.to_string()));
                continue;
            }
        }

        // Key-value pair, e.g. command: npx or env:
        if let Some((k, v)) = line_trimmed.split_once(':') {
            let key_str = k.trim().trim_matches('"').trim_matches('\'').to_string();
            let val_str = v.trim();

            let is_indented = line.starts_with("  ") || line.starts_with('\t');
            if is_indented && current_key.is_some() && !val_str.is_empty() && is_in_map {
                let clean_v = val_str.trim_matches('"').trim_matches('\'');
                current_map.insert(key_str, serde_json::Value::String(clean_v.to_string()));
                continue;
            }

            flush(
                &mut root,
                &mut current_key,
                &mut current_list,
                &mut current_map,
                &mut is_in_list,
                &mut is_in_map,
            );

            if val_str.is_empty() {
                current_key = Some(key_str.clone());
                if key_str == "env" {
                    is_in_map = true;
                }
            } else if (val_str.starts_with('[') && val_str.ends_with(']'))
                || (val_str.starts_with('{') && val_str.ends_with('}'))
            {
                if let Ok(parsed_json) = serde_json::from_str::<serde_json::Value>(val_str) {
                    root.insert(key_str, parsed_json);
                } else {
                    root.insert(key_str, serde_json::Value::String(val_str.to_string()));
                }
            } else {
                let clean_v = val_str.trim_matches('"').trim_matches('\'');
                root.insert(key_str, serde_json::Value::String(clean_v.to_string()));
            }
        }
    }

    flush(
        &mut root,
        &mut current_key,
        &mut current_list,
        &mut current_map,
        &mut is_in_list,
        &mut is_in_map,
    );

    serde_json::Value::Object(root)
}

/// Extract command executable and args from freeform config text or markdown
pub fn extract_command_from_raw_config(config: &str) -> Option<String> {
    let trimmed = config.trim();
    if trimmed.is_empty() {
        return None;
    }

    // 1. Backtick inline command: e.g. `wordcraft-cli mcp` or "list tools in command: `wordcraft-cli mcp`"
    if let Some(start_idx) = trimmed.find('`')
        && let Some(end_idx) = trimmed[start_idx + 1..].find('`')
    {
        let inside = trimmed[start_idx + 1..start_idx + 1 + end_idx].trim();
        if !inside.is_empty() && !inside.contains('\n') {
            return Some(inside.to_string());
        }
    }

    // 2. Code blocks (```bash ... ```, ```sh ... ```, etc.)
    for tag in &["```bash", "```sh", "```cmd", "```shell", "```"] {
        if let Some(start_idx) = trimmed.find(tag) {
            let after = &trimmed[start_idx + tag.len()..];
            if let Some(end_idx) = after.find("```") {
                let inside = after[..end_idx].trim();
                let first_line = inside.lines().next().unwrap_or("").trim();
                if !first_line.is_empty() {
                    return Some(first_line.to_string());
                }
            }
        }
    }

    // 3. Lines with prefixes like "command:", "command =", "run:", "exec:", "list tools in command:", "list tools:"
    for line in trimmed.lines() {
        let line_trimmed = line.trim();
        let lower = line_trimmed.to_lowercase();
        for prefix in &[
            "list tools in command:",
            "list tools from command:",
            "list tools in:",
            "list tools:",
            "command:",
            "command =",
            "cmd:",
            "run:",
            "exec:",
        ] {
            if let Some(pos) = lower.find(prefix) {
                let val = line_trimmed[pos + prefix.len()..]
                    .trim()
                    .trim_matches('`')
                    .trim_matches('"')
                    .trim_matches('\'')
                    .trim();
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
    }

    // 4. Single line plain text command if it doesn't look like JSON or Markdown heading
    if !trimmed.contains('\n')
        && !trimmed.starts_with('{')
        && !trimmed.starts_with('[')
        && !trimmed.starts_with('#')
    {
        let cleaned = trimmed
            .trim_matches('`')
            .trim_matches('"')
            .trim_matches('\'')
            .trim();
        if !cleaned.is_empty() {
            return Some(cleaned.to_string());
        }
    }

    None
}

impl McpServer {
    pub fn config_value(&self) -> serde_json::Value {
        let trimmed = self.config.trim();
        // 1. Direct JSON (legacy / backward-compatible)
        let parsed_val = if trimmed.starts_with('{')
            && let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed)
            && val.is_object()
        {
            Some(val)
        } else if let Some(block) = extract_execution_block(&self.config) {
            // 2. Extract execution block from Markdown
            let parsed = parse_yaml_or_json_value(block);
            if parsed.is_object() && !parsed.as_object().map(|m| m.is_empty()).unwrap_or(true) {
                Some(parsed)
            } else {
                None
            }
        } else {
            // 3. Fallback: attempt YAML/JSON directly on entire string
            let direct_parsed = parse_yaml_or_json_value(&self.config);
            if direct_parsed.is_object()
                && !direct_parsed
                    .as_object()
                    .map(|m| m.is_empty())
                    .unwrap_or(true)
            {
                Some(direct_parsed)
            } else {
                None
            }
        };

        let val = parsed_val.unwrap_or_else(|| serde_json::json!({}));
        Self::unwrap_nested_config(val, &self.name, &self.id)
    }

    /// Unwrap nested MCP configurations (e.g. {"mcpServers": { ... }}, {"servers": { ... }}, or {"<server_name>": { ... }})
    fn unwrap_nested_config(
        val: serde_json::Value,
        name_hint: &str,
        id_hint: &str,
    ) -> serde_json::Value {
        if !val.is_object() {
            return val;
        }

        // Check if config already has root fields
        if val.get("command").is_some()
            || val.get("url").is_some()
            || val.get("transport").is_some()
        {
            return val;
        }

        // Check wrapper keys: "mcpServers", "mcp_servers", "servers"
        for wrapper_key in &["mcpServers", "mcp_servers", "servers"] {
            if let Some(inner) = val.get(*wrapper_key)
                && inner.is_object()
            {
                // Check if inner matches name_hint or id_hint
                if let Some(server_obj) = inner.get(name_hint).or_else(|| inner.get(id_hint))
                    && server_obj.is_object()
                {
                    return server_obj.clone();
                }

                // If inner has a single key or child with command/url, pick it
                if let Some(obj_map) = inner.as_object() {
                    for (_k, v) in obj_map {
                        if v.is_object() && (v.get("command").is_some() || v.get("url").is_some()) {
                            return v.clone();
                        }
                    }
                    if obj_map.len() == 1
                        && let Some(first_val) = obj_map.values().next()
                        && first_val.is_object()
                    {
                        return first_val.clone();
                    }
                }
                return inner.clone();
            }
        }

        // Check if val has a single key matching server name or containing command/url
        if let Some(obj_map) = val.as_object() {
            if let Some(server_obj) = obj_map.get(name_hint).or_else(|| obj_map.get(id_hint))
                && server_obj.is_object()
            {
                return server_obj.clone();
            }
            if obj_map.len() == 1
                && let Some(first_val) = obj_map.values().next()
                && first_val.is_object()
                && (first_val.get("command").is_some() || first_val.get("url").is_some())
            {
                return first_val.clone();
            }
        }

        val
    }

    pub fn transport(&self) -> String {
        let v = self.config_value();
        v.get("transport")
            .and_then(|t| t.as_str())
            .unwrap_or_else(|| {
                if v.get("url").is_some() {
                    "sse"
                } else {
                    "stdio"
                }
            })
            .to_string()
    }

    pub fn command(&self) -> Option<String> {
        let v = self.config_value();
        if let Some(cmd) = v.get("command").and_then(|c| c.as_str()) {
            return Some(cmd.to_string());
        }
        extract_command_from_raw_config(&self.config)
    }

    pub fn url(&self) -> Option<String> {
        let v = self.config_value();
        v.get("url").and_then(|u| u.as_str()).map(|s| s.to_string())
    }

    pub fn args_json(&self) -> String {
        let v = self.config_value();
        v.get("args")
            .map(|a| serde_json::to_string(a).unwrap_or_else(|_| "[]".to_string()))
            .unwrap_or_else(|| "[]".to_string())
    }

    pub fn env_json(&self) -> String {
        let v = self.config_value();
        v.get("env")
            .map(|e| serde_json::to_string(e).unwrap_or_else(|_| "{}".to_string()))
            .unwrap_or_else(|| "{}".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_server_config_legacy_json() {
        let server = McpServer {
            id: "srv1".to_string(),
            name: "GitHub Server".to_string(),
            config: r#"{"transport":"stdio","command":"npx","args":["-y","@mcp/github"],"env":{"TOKEN":"abc"}}"#.to_string(),
            is_enabled: true,
            created_at: 0,
        };

        assert_eq!(server.transport(), "stdio");
        assert_eq!(server.command(), Some("npx".to_string()));
        assert_eq!(server.args_json(), r#"["-y","@mcp/github"]"#);
        assert_eq!(server.env_json(), r#"{"TOKEN":"abc"}"#);
    }

    #[test]
    fn test_mcp_server_config_markdown_exec_block() {
        let md = r#"
# My GitHub Server
Documentation and server notes.

## Execution
```exec
transport: stdio
command: npx
args:
  - "-y"
  - "@mcp/github"
env:
  GITHUB_TOKEN: "xyz123"
```

## Tools
Some notes here.
"#;
        let server = McpServer {
            id: "srv2".to_string(),
            name: "GitHub MD".to_string(),
            config: md.to_string(),
            is_enabled: true,
            created_at: 0,
        };

        assert_eq!(server.transport(), "stdio");
        assert_eq!(server.command(), Some("npx".to_string()));
        let args: Vec<String> = serde_json::from_str(&server.args_json()).unwrap();
        assert_eq!(args, vec!["-y", "@mcp/github"]);
        let env: serde_json::Value = serde_json::from_str(&server.env_json()).unwrap();
        assert_eq!(env["GITHUB_TOKEN"], "xyz123");
    }

    #[test]
    fn test_mcp_server_config_markdown_frontmatter() {
        let md = r#"---
transport: sse
url: http://localhost:8080/sse
---
# Remote Server
Description here
"#;
        let server = McpServer {
            id: "srv3".to_string(),
            name: "Remote MD".to_string(),
            config: md.to_string(),
            is_enabled: true,
            created_at: 0,
        };

        assert_eq!(server.transport(), "sse");
        assert_eq!(server.url(), Some("http://localhost:8080/sse".to_string()));
    }

    #[test]
    fn test_mcp_server_config_freeform_and_backticks() {
        let server1 = McpServer {
            id: "srv4".to_string(),
            name: "Wordcraft".to_string(),
            config: "list tools in command: `wordcraft-cli mcp`".to_string(),
            is_enabled: true,
            created_at: 0,
        };
        assert_eq!(server1.command(), Some("wordcraft-cli mcp".to_string()));

        let server2 = McpServer {
            id: "srv5".to_string(),
            name: "Wordcraft Plain".to_string(),
            config: "list tools in command: wordcraft-cli mcp".to_string(),
            is_enabled: true,
            created_at: 0,
        };
        assert_eq!(server2.command(), Some("wordcraft-cli mcp".to_string()));
    }
}
