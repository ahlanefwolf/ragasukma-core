use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::agent::mcp::{McpClient, McpServerConfig, McpToolDefinition};
use crate::agent::models::McpServer;
use crate::agent::repository::upsert_mcp_server_tool;
use crate::llm::client::LlmClient;

/// Tool information discovered either via LLM analysis or native JSON-RPC inspection
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscoveredTool {
    pub name: String,
    pub config: String,
}

/// System prompt for the LLM Tool Discovery Runner
pub const MCP_DISCOVERY_SYSTEM_PROMPT: &str = r#"You are an expert Model Context Protocol (MCP) Semantic Compiler and Tool Specialist.
Your task is to analyze the provided MCP server documentation and any raw tool schemas, then produce a clean, comprehensive Markdown specification for each available tool.

For each tool, wrap the output inside a `<tool name="TOOL_NAME">` block:

<tool name="tool_name">
# tool_name

A concise, high-level description of what the tool does and when the agent should call it.

## Parameters
- `param_name` (type, required/optional): Description of parameter and constraints.

## Execution Contract
```json
{
  "method": "tools/call",
  "name": "tool_name"
}
```
</tool>

Rules:
1. Every tool must have a distinct `<tool name="...">` and `</tool>` tag.
2. The inside of the tag must be valid Markdown.
3. Be accurate regarding parameter names, types, and descriptions.
4. If no tools are available or detectable, return no `<tool>` tags.
"#;

/// Build the user prompt for the LLM runner
#[must_use]
pub fn build_discovery_prompt(
    server: &McpServer,
    raw_tools: Option<&[McpToolDefinition]>,
) -> String {
    let mut prompt = format!(
        "Server Name: {}\n\nServer Configuration & Documentation:\n```markdown\n{}\n```\n\n",
        server.name, server.config
    );

    if let Some(tools) = raw_tools {
        if !tools.is_empty() {
            prompt.push_str("Raw Tools Discovered via Native MCP Handshake (tools/list):\n```json\n");
            let json_str = serde_json::to_string_pretty(tools).unwrap_or_else(|_| "[]".to_string());
            prompt.push_str(&json_str);
            prompt.push_str("\n```\n\n");
        }
    }

    prompt.push_str("Please extract and generate the Markdown specification for every tool provided above.");
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
                    tools.push(DiscoveredTool {
                        name,
                        config: tool_content,
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
                    tools.push(DiscoveredTool { name, config: body });
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
            tools.push(DiscoveredTool { name, config: body });
        }
    }

    tools
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
    if let Some(props) = tool.input_schema.get("properties").and_then(|p| p.as_object()) {
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

    md.push_str("\n## Execution Contract\n```json\n{\n");
    md.push_str(&format!("  \"method\": \"tools/call\",\n  \"name\": \"{}\"\n", tool.name));
    md.push_str("}\n```\n");

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
        is_enabled: server.is_enabled == 1,
    })
}

/// Try to perform native handshake and list tools from the MCP server
pub async fn try_native_list_tools(server: &McpServer) -> Result<Vec<McpToolDefinition>, String> {
    let config = server_to_config(server)
        .ok_or_else(|| "Server does not have an active runtime transport configuration".to_string())?;

    let client = McpClient::new(config);
    client.initialize().await?;
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
            info!("Native handshake retrieved {} tools for server {}", tools.len(), server.name);
            Some(tools)
        }
        Err(e) => {
            warn!("Native handshake failed or skipped for server {}: {}", server.name, e);
            None
        }
    };

    // 2. If model runner is provided or requested, invoke LLM
    if let Some(model) = model_name {
        let user_prompt = build_discovery_prompt(server, raw_tools.as_deref());
        let sys_prompt = MCP_DISCOVERY_SYSTEM_PROMPT.to_string();
        let target_model = model.to_string();

        let llm_res: Result<String, String> = tokio::task::spawn_blocking(move || {
            let mut client = LlmClient::new();
            client.model = target_model;
            client.set_system_prompt(sys_prompt);
            client
                .chat_stream(&user_prompt, |_| Ok(()))
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| format!("LLM runner task execution error: {e}"))?;

        match llm_res {
            Ok(output) => {
                let parsed = parse_tools_from_response(&output);
                if !parsed.is_empty() {
                    return Ok(parsed);
                }
                warn!("LLM discovery returned output but parsed 0 tools; evaluating fallbacks.");
            }
            Err(e) => {
                warn!("LLM discovery failed: {}. Falling back to native tools if available.", e);
            }
        }
    }

    // 3. Fallback: If native tools exist, convert them to Markdown specifications
    if let Some(tools) = raw_tools {
        if !tools.is_empty() {
            let converted: Vec<DiscoveredTool> =
                tools.iter().map(tool_definition_to_markdown).collect();
            return Ok(converted);
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

## Execution Contract
```json
{
  "method": "tools/call",
  "name": "query_database"
}
```
</tool>

<tool name="list_tables">
# list_tables

List all available tables.

## Execution Contract
```json
{
  "method": "tools/call",
  "name": "list_tables"
}
```
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
        assert!(tool.config.contains("- `a` (number, required): First number"));
        assert!(tool.config.contains("- `b` (number, required): Second number"));
    }
}
