use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::agent::mcp::{McpClient, McpServerConfig, McpToolDefinition};
use crate::agent::models::McpServer;
use crate::llm::client::LlmClient;
use crate::repository::agent::upsert_mcp_server_tool;

/// Tool information discovered either via LLM analysis or native JSON-RPC inspection
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscoveredTool {
    pub name: String,
    pub config: String,
}

/// System prompt for the LLM Tool Discovery Runner (Command Only)
pub const MCP_DISCOVERY_SYSTEM_PROMPT: &str = r#"You are a CLI command generator for external MCP tools.
Your ONLY task is to generate the single-line shell command template needed to execute each tool, using `{param}` placeholders.

Output format:
<tool name="tool_name">command --flag {param}</tool>

Rules:
1. Output ONLY the single-line shell command inside <tool name="...">...</tool>.
2. Do NOT write markdown headings, explanations, parameters lists, or usage guides.
3. Every command template must be a single runnable line."#;

/// Build the user prompt for the LLM runner
#[must_use]
pub fn build_discovery_prompt(
    server: &McpServer,
    raw_tools: Option<&[McpToolDefinition]>,
) -> String {
    let mut prompt = format!(
        "Server Name: {}\nServer ID: {}\n\nServer Configuration:\n```\n{}\n```\n\n",
        server.name, server.id, server.config
    );

    if let Some(tools) = raw_tools
        && !tools.is_empty()
    {
        prompt.push_str("Tools Discovered via MCP Handshake (tools/list):\n");
        for tool in tools {
            let desc = tool.description.as_deref().unwrap_or("No description");
            prompt.push_str(&format!("- Tool `{}`: {}\n", tool.name, desc));
            if let Some(props) = tool
                .input_schema
                .get("properties")
                .and_then(|p| p.as_object())
            {
                for (pname, pval) in props {
                    let ptype = pval.get("type").and_then(|t| t.as_str()).unwrap_or("any");
                    let pdesc = pval
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or("");
                    if pdesc.is_empty() {
                        prompt.push_str(&format!("  * `{pname}` ({ptype})\n"));
                    } else {
                        prompt.push_str(&format!("  * `{pname}` ({ptype}): {pdesc}\n"));
                    }
                }
            }
        }
        prompt.push('\n');
    }

    prompt.push_str(
        "Study the MCP server configuration (Markdown skill description from the user) above.\n\
        You are demanded to formulate and generate the complete specification for each tool \
        provided by this MCP server so they can be registered into the mcp_server_tools database.\n\
        Every tool must include a `## Command Template` section with EXACTLY ONE SINGLE LINE runnable shell command \
        using `{param}` placeholders (derive it from the configuration, executables, or scripts \
        the user described). The command template MUST NOT contain multiple lines or linebreaks.\n\
        Wrap each tool inside `<tool name=\"tool_name\"> ... </tool>` tags.",
    );
    prompt
}

/// Build a focused user prompt for the LLM runner to generate a rich specification for ONE single tool.
/// This gives the raw `mcp_servers.config` to the LLM so it learns how the server runs.
#[must_use]
pub fn build_single_tool_prompt(server: &McpServer, tool: &McpToolDefinition) -> String {
    let mut prompt = format!(
        "Server Name: {}\n\nServer Configuration (`mcp_servers.config`):\n```\n{}\n```\n\nTarget Tool: {}\n",
        server.name,
        server.config.trim(),
        tool.name
    );

    if let Some(ref desc) = tool.description {
        prompt.push_str(&format!("Description: {}\n", desc));
    }
    if let Some(props) = tool
        .input_schema
        .get("properties")
        .and_then(|p| p.as_object())
    {
        let prop_names: Vec<String> = props.keys().cloned().collect();
        prompt.push_str(&format!("Parameters: {}\n", prop_names.join(", ")));
    }

    prompt.push_str(&format!(
        "\nBased on the raw server configuration above, generate ONLY the single-line shell command template to execute `{}` using `{{param}}` placeholders.\n\
        Format:\n\
        <tool name=\"{}\">command --arg {{param}}</tool>",
        tool.name, tool.name
    ));
    prompt
}

/// Parse tools from LLM response supporting `<tool name="...">` tags,
/// `### TOOL: name` headings, or JSON fallback.
#[must_use]
pub fn parse_tools_from_response(response: &str) -> Vec<DiscoveredTool> {
    let mut tools = Vec::new();

    // 1. Check for XML-style `<tool name="...">` tags
    let mut remaining = response;
    while let Some(start_tag_idx) = remaining.find("<tool") {
        let after_start = &remaining[start_tag_idx..];
        if let Some(tag_close_idx) = after_start.find('>') {
            let tag_header = &after_start[..tag_close_idx];
            let name = if let Some(name_attr) = tag_header.find("name=\"") {
                let after_name = &tag_header[name_attr + 6..];
                if let Some(quote_end) = after_name.find('"') {
                    after_name[..quote_end].trim().to_string()
                } else {
                    "unknown_tool".to_string()
                }
            } else if let Some(name_attr) = tag_header.find("name='") {
                let after_name = &tag_header[name_attr + 6..];
                if let Some(quote_end) = after_name.find('\'') {
                    after_name[..quote_end].trim().to_string()
                } else {
                    "unknown_tool".to_string()
                }
            } else {
                "unknown_tool".to_string()
            };

            let content_start = &after_start[tag_close_idx + 1..];
            if let Some(end_tag_idx) = content_start.find("</tool>") {
                let tool_content = content_start[..end_tag_idx].trim().to_string();
                if !name.is_empty() && !tool_content.is_empty() {
                    let sanitized = sanitize_command_template_to_single_line(&tool_content);
                    tools.push(DiscoveredTool {
                        name,
                        config: sanitized,
                    });
                }
                remaining = &content_start[end_tag_idx + 7..];
            } else {
                break;
            }
        } else {
            break;
        }
    }

    if !tools.is_empty() {
        return tools;
    }

    // 2. Check for `### TOOL: <name>` style delimiters
    let lines: Vec<&str> = response.lines().collect();
    let mut current_tool: Option<(String, Vec<String>)> = None;

    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with("### TOOL:") || trimmed.starts_with("## TOOL:") {
            if let Some((name, body_lines)) = current_tool.take() {
                let body = body_lines.join("\n").trim().to_string();
                if !name.is_empty() && !body.is_empty() {
                    tools.push(DiscoveredTool {
                        name,
                        config: sanitize_command_template_to_single_line(&body),
                    });
                }
            }
            let name_part = if let Some(rest) = trimmed.strip_prefix("### TOOL:") {
                rest
            } else {
                trimmed.strip_prefix("## TOOL:").unwrap_or_default()
            };
            let name = name_part.trim().to_string();
            current_tool = Some((name, Vec::new()));
        } else if let Some((_, ref mut body_lines)) = current_tool {
            body_lines.push(line.to_string());
        }
    }

    if let Some((name, body_lines)) = current_tool {
        let body = body_lines.join("\n").trim().to_string();
        if !name.is_empty() && !body.is_empty() {
            tools.push(DiscoveredTool {
                name,
                config: sanitize_command_template_to_single_line(&body),
            });
        }
    }

    // 3. Fallback: Check if response is raw JSON array of tools
    if tools.is_empty() {
        let trimmed_resp = response.trim();
        let json_slice = if let Some(start) = trimmed_resp.find('[') {
            if let Some(end) = trimmed_resp.rfind(']') {
                &trimmed_resp[start..=end]
            } else {
                trimmed_resp
            }
        } else {
            trimmed_resp
        };

        if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_slice)
            && let Some(arr) = val.as_array()
        {
            for item in arr {
                if let Some(name) = item.get("name").and_then(|n| n.as_str()) {
                    let config = item
                        .get("description")
                        .or_else(|| item.get("config"))
                        .and_then(|c| c.as_str())
                        .unwrap_or("");
                    let tool_md = if config.starts_with('#') {
                        config.to_string()
                    } else {
                        format!("# {}\n\n{}\n", name, config)
                    };
                    tools.push(DiscoveredTool {
                        name: name.to_string(),
                        config: sanitize_command_template_to_single_line(&tool_md),
                    });
                }
            }
        }
    }

    tools
}

/// Sanitizes a Markdown tool specification so that the `## Command Template`
/// section contains strictly a single-line execution command inside its code block.
#[must_use]
pub fn sanitize_command_template_to_single_line(config: &str) -> String {
    let mut result = Vec::new();
    let mut in_command_template = false;
    let mut in_code_block = false;
    let mut code_block_fence = String::new();
    let mut command_lines = Vec::new();

    for line in config.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("## ") {
            if in_command_template && in_code_block {
                // Closing unclosed code block before next header
                let single_line = collapse_command_lines(&command_lines);
                result.push(single_line);
                result.push(code_block_fence.clone());
                in_code_block = false;
                command_lines.clear();
            }
            in_command_template = trimmed.to_lowercase().contains("command template");
            result.push(line.to_string());
            continue;
        }

        if in_command_template {
            if trimmed.starts_with("```") {
                if !in_code_block {
                    in_code_block = true;
                    code_block_fence = trimmed.to_string();
                    result.push(line.to_string());
                    command_lines.clear();
                } else {
                    // Exiting code block: collapse collected command lines into exactly one single line
                    let single_line = collapse_command_lines(&command_lines);
                    if !single_line.is_empty() {
                        result.push(single_line);
                    }
                    result.push(line.to_string());
                    in_code_block = false;
                    command_lines.clear();
                }
                continue;
            }

            if in_code_block {
                if !trimmed.is_empty() {
                    command_lines.push(trimmed);
                }
                continue;
            }
        }

        result.push(line.to_string());
    }

    if in_code_block {
        let single_line = collapse_command_lines(&command_lines);
        if !single_line.is_empty() {
            result.push(single_line);
        }
        result.push(code_block_fence);
    }

    result.join("\n")
}

fn collapse_command_lines(lines: &[&str]) -> String {
    let mut joined = String::new();
    for (i, line) in lines.iter().enumerate() {
        let mut clean = line.trim();
        // Remove trailing shell continuation backslash if present
        if clean.ends_with('\\') {
            clean = clean[..clean.len() - 1].trim();
        }
        if i > 0 && !joined.is_empty() && !clean.is_empty() {
            joined.push(' ');
        }
        joined.push_str(clean);
    }
    joined
}

/// Fallback converter: converts native `McpToolDefinition` JSON into clean Markdown spec
#[must_use]
pub fn tool_definition_to_markdown(tool: &McpToolDefinition) -> DiscoveredTool {
    let mut md = format!("# {}\n\n", tool.name);

    if let Some(ref desc) = tool.description {
        md.push_str(desc);
        md.push_str("\n\n");
    } else {
        md.push_str("No description provided by MCP server.\n\n");
    }

    md.push_str("## Parameters\n");
    if let Some(props) = tool
        .input_schema
        .get("properties")
        .and_then(|p| p.as_object())
    {
        let required_list: Vec<&str> = tool
            .input_schema
            .get("required")
            .and_then(|r| r.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();

        for (prop_name, prop_val) in props {
            let prop_type = prop_val
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("any");
            let is_req = required_list.contains(&prop_name.as_str());
            let req_str = if is_req { "required" } else { "optional" };
            let prop_desc = prop_val
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("");

            if prop_desc.is_empty() {
                md.push_str(&format!("- `{prop_name}` ({prop_type}, {req_str})\n"));
            } else {
                md.push_str(&format!(
                    "- `{prop_name}` ({prop_type}, {req_str}): {prop_desc}\n"
                ));
            }
        }
    } else {
        md.push_str("No parameters required.\n");
    }

    md.push_str("\n## Command Template\n```bash\n");
    let mut cmd_parts = vec![tool.name.clone()];
    if let Some(props) = tool
        .input_schema
        .get("properties")
        .and_then(|p| p.as_object())
    {
        for (prop_name, _) in props {
            cmd_parts.push(format!("--{prop_name} \"{{{prop_name}}}\""));
        }
    }
    md.push_str(&cmd_parts.join(" "));
    md.push_str("\n```\n\n## Usage Guidelines\n");
    md.push_str(&format!(
        "External CLI command for `{}`. Execute directly via `workspace_run_command` with the parameters defined above.\n",
        tool.name
    ));

    DiscoveredTool {
        name: tool.name.clone(),
        config: md,
    }
}

/// Convert `McpServer` to `McpServerConfig` for runtime client connection
#[must_use]
pub fn server_to_config(server: &McpServer) -> Option<McpServerConfig> {
    let transport = server.transport();
    if transport == "disabled" {
        return None;
    }

    let args: Vec<String> = serde_json::from_str(&server.args_json()).unwrap_or_default();
    let env: std::collections::HashMap<String, String> =
        serde_json::from_str(&server.env_json()).unwrap_or_default();

    Some(McpServerConfig {
        id: server.id.clone(),
        name: server.name.clone(),
        transport,
        command: server.command(),
        args,
        env,
        url: server.url(),
        is_enabled: server.is_enabled,
    })
}

/// Try to perform native handshake and list tools from the MCP server
pub async fn try_native_list_tools(server: &McpServer) -> Result<Vec<McpToolDefinition>, String> {
    let config = server_to_config(server).ok_or_else(|| {
        "Server does not have an active runtime transport configuration".to_string()
    })?;

    let client = McpClient::new(config);
    let _ = client.initialize().await;
    client.list_tools().await
}

/// Discover tools using native handshake and/or LLM Semantic Runner
pub async fn discover_tools(
    server: &McpServer,
    model_name: Option<&str>,
) -> Result<Vec<DiscoveredTool>, String> {
    // 1. Attempt native handshake if server has runtime configuration
    let raw_tools = match try_native_list_tools(server).await {
        Ok(tools) => {
            info!(
                "Native handshake retrieved {} tools for server {}",
                tools.len(),
                server.name
            );
            Some(tools)
        }
        Err(e) => {
            warn!(
                "Native handshake failed or skipped for server {}: {}",
                server.name, e
            );
            None
        }
    };

    // 2. Resolve LLM model runner: use explicitly provided model or default active model
    let target_model = match model_name {
        Some(m) if !m.trim().is_empty() => Some(m.trim().to_string()),
        _ => {
            let user_cfg = crate::config::load_user_config();
            if !user_cfg.llm_model.trim().is_empty() {
                Some(user_cfg.llm_model)
            } else {
                None
            }
        }
    };

    // OS-aware shell description, same pattern as executor/prompt.rs: on Windows
    // PowerShell with ';' separator (no '&&'), on Unix POSIX shell with '&&'.
    #[cfg(target_os = "windows")]
    let shell_desc = "OS: Windows. Shell: PowerShell. Use native PowerShell syntax for scripts and paths. For multiple commands in one execution, use ';' as separator (do not use '&&' as Windows PowerShell 5.1 does not support it). When a command needs curl, write `curl.exe` (the bare `curl` is an alias for Invoke-WebRequest and will fail). Script runtimes: Node.js (`node`) or Python (`python` / `py -3`).";

    #[cfg(not(target_os = "windows"))]
    let shell_desc = "OS: macOS/Linux. Shell: POSIX shell. Use standard Unix shell syntax; chain sequential commands with '&&'. Script runtimes: Node.js (`node`) or Python (`python3`).";

    let sys_prompt = MCP_DISCOVERY_SYSTEM_PROMPT.replace("{shell_desc}", shell_desc);

    // 2. If native MCP handshake returned raw_tools, instantly map each tool definition
    // directly to its clean markdown specification without waiting for slow serial LLM calls.
    if let Some(raw_list) = raw_tools
        && !raw_list.is_empty()
    {
        info!(
            "Instant native registration: converting {} tools directly for server '{}'",
            raw_list.len(),
            server.name
        );
        let final_tools: Vec<DiscoveredTool> =
            raw_list.iter().map(tool_definition_to_markdown).collect();

        return Ok(final_tools);
    }

    // 3. Fallback for document-only servers without a running transport binary:
    // Generate specifications in batch using the skill configuration document.
    if let Some(model) = target_model {
        let user_prompt = build_discovery_prompt(server, None);
        let sys_prompt_clone = sys_prompt.clone();

        let llm_res: Result<String, String> = tokio::task::spawn_blocking(move || {
            let mut client = LlmClient::new();
            client.model = model;
            client.set_system_prompt(sys_prompt_clone);
            client
                .chat_stream(&user_prompt, |_| Ok(()))
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| format!("LLM runner task execution error: {e}"))?;

        if let Ok(output) = llm_res {
            let parsed = parse_tools_from_response(&output);
            if !parsed.is_empty() {
                return Ok(parsed);
            }
            warn!(
                "LLM discovery returned output but parsed 0 tools; evaluating markdown fallbacks."
            );
        }
    }

    // 4. Fallback: Parse tools directly from the server markdown content
    let parsed_from_doc = parse_tools_from_response(&server.config);
    if !parsed_from_doc.is_empty() {
        return Ok(parsed_from_doc);
    }

    Ok(Vec::new())
}

/// Synchronize tools for an MCP server: discover tools and upsert into SQLite database
pub async fn sync_server_tools(
    server: &McpServer,
    model_name: Option<&str>,
) -> Result<Vec<DiscoveredTool>, String> {
    let tools = discover_tools(server, model_name).await?;

    for tool in &tools {
        upsert_mcp_server_tool(&server.id, &tool.name, &tool.config)
            .await
            .map_err(|e| format!("Failed to upsert tool {}: {e}", tool.name))?;
        info!(
            "Inserted/upserted tool '{}' for MCP server '{}' ({}) into mcp_server_tools",
            tool.name, server.name, server.id
        );
    }

    Ok(tools)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tools_xml_style() {
        let response = r#"
Here are the tools found:

<tool name="query_database">
# query_database

Execute a SQL query against the SQLite database.

## Parameters
- `sql` (string, required): The SQL query string.

## Usage Guidelines
Agnostic database tool.
</tool>

<tool name="list_tables">
# list_tables

List all available tables.

## Usage Guidelines
List tables in the database.
</tool>
"#;

        let tools = parse_tools_from_response(response);
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "query_database");
        assert!(tools[0].config.contains("Execute a SQL query"));
        assert_eq!(tools[1].name, "list_tables");
        assert!(tools[1].config.contains("List all available tables"));
    }

    #[test]
    fn test_parse_tools_markdown_heading_style() {
        let response = r#"
### TOOL: search_docs
# search_docs
Search internal documentation.

### TOOL: delete_file
# delete_file
Delete a file on disk.
"#;

        let tools = parse_tools_from_response(response);
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "search_docs");
        assert_eq!(tools[1].name, "delete_file");
    }

    #[test]
    fn test_tool_definition_to_markdown() {
        let raw = McpToolDefinition {
            name: "calculate_sum".to_string(),
            description: Some("Add two numbers together".to_string()),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "a": { "type": "number", "description": "First number" },
                    "b": { "type": "number", "description": "Second number" }
                },
                "required": ["a", "b"]
            }),
        };

        let tool = tool_definition_to_markdown(&raw);
        assert_eq!(tool.name, "calculate_sum");
        assert!(tool.config.contains("# calculate_sum"));
        assert!(tool.config.contains("Add two numbers together"));
        assert!(
            tool.config
                .contains("- `a` (number, required): First number")
        );
        assert!(
            tool.config
                .contains("- `b` (number, required): Second number")
        );
        assert!(!tool.config.contains("```json"));
    }

    #[test]
    fn test_sanitize_command_template_to_single_line() {
        let multiline_spec = r#"# sample_tool

Description here.

## Parameters
- `param` (string, required): A parameter.

## Command Template
```bash
printf '%s\n' \
  '{"jsonrpc":"2.0"}' \
  | figcraft-cli mcp
```

## Usage Guidelines
Use wisely.
"#;

        let sanitized = sanitize_command_template_to_single_line(multiline_spec);
        assert!(
            sanitized.contains(
                "```bash\nprintf '%s\\n' '{\"jsonrpc\":\"2.0\"}' | figcraft-cli mcp\n```"
            )
        );
        assert!(sanitized.contains("## Usage Guidelines"));
    }

    #[test]
    fn test_build_single_tool_prompt() {
        let server = McpServer {
            id: "figma_mcp".to_string(),
            name: "Figma Server".to_string(),
            config: "Figma design integration CLI".to_string(),
            is_enabled: true,
            created_at: 0,
        };

        let raw_tool = McpToolDefinition {
            name: "inspect_node".to_string(),
            description: Some("Inspects layout and styling of a Figma node".to_string()),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "file_key": { "type": "string", "description": "Figma file key" },
                    "node_id": { "type": "string", "description": "Target node id" }
                },
                "required": ["file_key", "node_id"]
            }),
        };

        let prompt = build_single_tool_prompt(&server, &raw_tool);
        assert!(prompt.contains("Tool: inspect_node"));
        assert!(prompt.contains("Description: Inspects layout and styling of a Figma node"));
        assert!(prompt.contains("file_key") && prompt.contains("node_id"));
        assert!(prompt.contains("<tool name=\"inspect_node\">"));
    }
}
