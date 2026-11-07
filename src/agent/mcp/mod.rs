// Agent MCP (Model Context Protocol) integration module
// Fully language-agnostic implementation supporting stdio and HTTP/SSE JSON-RPC 2.0 transports.

pub mod discovery;

pub mod types {
    use serde::{Deserialize, Serialize};
    use std::collections::HashMap;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct McpToolDefinition {
        pub name: String,
        pub description: Option<String>,
        #[serde(rename = "inputSchema", default = "default_schema")]
        pub input_schema: serde_json::Value,
    }

    fn default_schema() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {}
        })
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct McpCallToolRequest {
        pub name: String,
        #[serde(default)]
        pub arguments: serde_json::Value,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct McpToolResult {
        pub is_error: bool,
        pub content: Vec<serde_json::Value>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct McpServerConfig {
        pub id: String,
        pub name: String,
        pub transport: String, // "stdio", "sse", "http"
        pub command: Option<String>,
        #[serde(default)]
        pub args: Vec<String>,
        #[serde(default)]
        pub env: HashMap<String, String>,
        pub url: Option<String>,
        pub is_enabled: bool,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct McpTestConnectionResult {
        pub success: bool,
        pub server_name: String,
        pub transport: String,
        pub tools_found: usize,
        pub tools: Vec<String>,
        pub error: Option<String>,
        pub latency_ms: u64,
    }
}

use std::collections::HashMap;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

pub use types::*;

/// Resolves executable command and arguments directly via system PATH.
fn resolve_executable(
    command_str: &str,
    args: &[String],
) -> (String, Vec<String>, Option<std::path::PathBuf>) {
    (command_str.to_string(), args.to_vec(), None)
}

/// Execute JSON-RPC 2.0 request over stdio transport
async fn call_stdio_rpc(
    command_str: &str,
    args: &[String],
    env: &HashMap<String, String>,
    request_json: &serde_json::Value,
    timeout_duration: Duration,
) -> Result<serde_json::Value, String> {
    let payload = serde_json::to_string(request_json).map_err(|e| e.to_string())?;

    let (exec_cmd, exec_args, extra_path) = resolve_executable(command_str, args);

    let mut cmd = Command::new(&exec_cmd);
    cmd.args(&exec_args);
    for (k, v) in env {
        cmd.env(k, v);
    }

    // Ensure PATH includes the resolved binary directory so sub-processes can run tools
    if let Some(bin_dir) = extra_path {
        let current_path = std::env::var("PATH").unwrap_or_default();
        let bin_dir_str = bin_dir.to_string_lossy();
        if !current_path.contains(&*bin_dir_str) {
            #[cfg(target_os = "windows")]
            let sep = ";";
            #[cfg(not(target_os = "windows"))]
            let sep = ":";
            cmd.env("PATH", format!("{}{}{}", bin_dir_str, sep, current_path));
        }
    }
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn process '{}': {}", command_str, e))?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "Failed to capture stdin of MCP process".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Failed to capture stdout of MCP process".to_string())?;

    let line_writer = tokio::spawn(async move {
        stdin
            .write_all(payload.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        stdin.write_all(b"\n").await.map_err(|e| e.to_string())?;
        stdin.flush().await.map_err(|e| e.to_string())?;
        Ok::<(), String>(())
    });

    let mut reader = BufReader::new(stdout).lines();

    let rpc_result = tokio::select! {
        res = line_writer => {
            if let Err(e) = res {
                let _ = child.kill().await;
                return Err(format!("Failed writing to stdin: {:?}", e));
            }
            // Read response line
            tokio::select! {
                line = reader.next_line() => {
                    match line {
                        Ok(Some(line_str)) => {
                            serde_json::from_str::<serde_json::Value>(&line_str)
                                .map_err(|e| format!("Invalid JSON received from MCP process: {} (raw: {})", e, line_str))
                        }
                        Ok(None) => Err("MCP process closed stdout without response".to_string()),
                        Err(e) => Err(format!("Error reading from MCP stdout: {}", e)),
                    }
                }
                _ = tokio::time::sleep(timeout_duration) => {
                    let _ = child.kill().await;
                    Err("Timed out waiting for MCP process response".to_string())
                }
            }
        }
        _ = tokio::time::sleep(timeout_duration) => {
            let _ = child.kill().await;
            Err("Timed out spawning or writing to MCP process".to_string())
        }
    };

    let _ = child.kill().await;
    rpc_result
}

/// Execute JSON-RPC 2.0 request over HTTP/SSE endpoint.
/// Supports both Direct JSON-RPC HTTP and standard MCP SSE handshake.
async fn call_http_rpc(
    url: &str,
    request_json: &serde_json::Value,
    timeout_duration: Duration,
) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::builder()
        .timeout(timeout_duration)
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

    // 1. First attempt direct POST (standard for Bun-MCP & direct JSON-RPC 2.0)
    let resp = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(request_json)
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            let body: serde_json::Value = r
                .json()
                .await
                .map_err(|e| format!("Failed to parse HTTP JSON response: {}", e))?;
            Ok(body)
        }
        Ok(r) if r.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED && url.ends_with("/sse") => {
            // Server might require standard SSE handshake via GET first
            if let Ok(mut sse_resp) = client
                .get(url)
                .header("Accept", "text/event-stream")
                .send()
                .await
                && sse_resp.status().is_success()
            {
                while let Ok(Some(chunk)) = sse_resp.chunk().await {
                    let text = String::from_utf8_lossy(&chunk);
                    for line in text.lines() {
                        if line.starts_with("data:") {
                            let endpoint = line.trim_start_matches("data:").trim();
                            let resolved_url = if endpoint.starts_with("http://")
                                || endpoint.starts_with("https://")
                            {
                                endpoint.to_string()
                            } else {
                                let base = url.trim_end_matches("/sse");
                                let clean_path = if endpoint.starts_with('/') {
                                    endpoint.to_string()
                                } else {
                                    format!("/{}", endpoint)
                                };
                                format!("{}{}", base, clean_path)
                            };

                            let post_resp = client
                                .post(&resolved_url)
                                .header("Content-Type", "application/json")
                                .json(request_json)
                                .send()
                                .await
                                .map_err(|e| format!("POST to SSE endpoint failed: {}", e))?;

                            let body: serde_json::Value = post_resp
                                .json()
                                .await
                                .map_err(|e| format!("Failed to parse SSE JSON response: {}", e))?;
                            return Ok(body);
                        }
                    }
                }
            }
            Err(format!(
                "MCP HTTP server responded with status: {}",
                r.status()
            ))
        }
        Ok(r) => Err(format!(
            "MCP HTTP server responded with status: {}",
            r.status()
        )),
        Err(e) => Err(format!("HTTP request failed to {}: {}", url, e)),
    }
}

/// Generic MCP client instance that handles tool listing and execution
pub struct McpClient {
    pub config: McpServerConfig,
    pub timeout: Duration,
}

impl McpClient {
    pub fn new(config: McpServerConfig) -> Self {
        Self {
            config,
            timeout: Duration::from_secs(15),
        }
    }

    /// Run initialization handshake
    pub async fn initialize(&self) -> Result<serde_json::Value, String> {
        let init_req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "clientInfo": {
                    "name": "ragasukma-agent",
                    "version": "1.0.0"
                }
            }
        });

        self.send_rpc(&init_req).await
    }

    /// List available tools from the MCP server
    pub async fn list_tools(&self) -> Result<Vec<McpToolDefinition>, String> {
        let list_req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        });

        let resp = self.send_rpc(&list_req).await?;

        if let Some(err) = resp.get("error") {
            return Err(format!("MCP tools/list error: {}", err));
        }

        let tools_val = resp
            .get("result")
            .and_then(|r| r.get("tools"))
            .cloned()
            .unwrap_or(serde_json::Value::Array(vec![]));

        let tools: Vec<McpToolDefinition> = serde_json::from_value(tools_val)
            .map_err(|e| format!("Failed to parse MCP tools definition: {}", e))?;

        Ok(tools)
    }

    /// Call a specific tool on the MCP server
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<McpToolResult, String> {
        let call_req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": name,
                "arguments": arguments
            }
        });

        let resp = self.send_rpc(&call_req).await?;

        if let Some(err) = resp.get("error") {
            return Ok(McpToolResult {
                is_error: true,
                content: vec![err.clone()],
            });
        }

        let result_val = resp.get("result").unwrap_or(&serde_json::Value::Null);
        let is_error = result_val
            .get("isError")
            .and_then(|b| b.as_bool())
            .unwrap_or(false);
        let content = result_val
            .get("content")
            .and_then(|c| c.as_array())
            .cloned()
            .unwrap_or_else(|| vec![result_val.clone()]);

        Ok(McpToolResult { is_error, content })
    }

    /// Dispatch JSON-RPC call based on configured transport
    async fn send_rpc(&self, request: &serde_json::Value) -> Result<serde_json::Value, String> {
        let transport = self.config.transport.to_lowercase();
        match transport.as_str() {
            "stdio" => {
                let cmd = self.config.command.as_deref().ok_or_else(|| {
                    "MCP stdio transport requires a 'command' to execute".to_string()
                })?;
                call_stdio_rpc(
                    cmd,
                    &self.config.args,
                    &self.config.env,
                    request,
                    self.timeout,
                )
                .await
            }
            "sse" | "http" => {
                let url =
                    self.config.url.as_deref().ok_or_else(|| {
                        "MCP HTTP/SSE transport requires a valid 'url'".to_string()
                    })?;
                call_http_rpc(url, request, self.timeout).await
            }
            other => Err(format!(
                "Unsupported MCP transport '{}'. Supported: stdio, sse, http",
                other
            )),
        }
    }

    /// Test server connectivity and retrieve available tools
    pub async fn test_connection(&self) -> McpTestConnectionResult {
        let start = Instant::now();
        let name = self.config.name.clone();
        let transport = self.config.transport.clone();

        match self.list_tools().await {
            Ok(tools) => {
                let tool_names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
                McpTestConnectionResult {
                    success: true,
                    server_name: name,
                    transport,
                    tools_found: tools.len(),
                    tools: tool_names,
                    error: None,
                    latency_ms: start.elapsed().as_millis() as u64,
                }
            }
            Err(e) => {
                // If tools/list failed, check if initialize succeeds
                match self.initialize().await {
                    Ok(_) => McpTestConnectionResult {
                        success: true,
                        server_name: name,
                        transport,
                        tools_found: 0,
                        tools: vec![],
                        error: Some(format!("Handshake ok, but tools/list returned: {}", e)),
                        latency_ms: start.elapsed().as_millis() as u64,
                    },
                    Err(init_err) => McpTestConnectionResult {
                        success: false,
                        server_name: name,
                        transport,
                        tools_found: 0,
                        tools: vec![],
                        error: Some(format!("Connection failed: {}", init_err)),
                        latency_ms: start.elapsed().as_millis() as u64,
                    },
                }
            }
        }
    }
}

impl From<&crate::agent::models::McpServer> for McpServerConfig {
    fn from(s: &crate::agent::models::McpServer) -> Self {
        let args: Vec<String> = serde_json::from_str(&s.args_json()).unwrap_or_default();
        let env: HashMap<String, String> = serde_json::from_str(&s.env_json()).unwrap_or_default();
        Self {
            id: s.id.clone(),
            name: s.name.clone(),
            transport: s.transport(),
            command: s.command(),
            args,
            env,
            url: s.url(),
            is_enabled: s.is_enabled == 1,
        }
    }
}

/// Represents an active MCP tool discovered from an active project MCP server or DB cache
#[derive(Debug, Clone)]
pub struct ActiveMcpTool {
    pub server_id: String,
    pub server_name: String,
    pub original_name: String,
    pub namespaced_name: String,
    pub definition: McpToolDefinition,
    pub server_config: McpServerConfig,
    pub markdown_spec: Option<String>,
}

/// Discovers tools from all active MCP servers for a project, prioritizing synced tools in DB
/// while falling back to live `tools/list` handshake when available.
pub async fn resolve_active_mcp_tools(
    servers: &[crate::agent::models::McpServer],
) -> Vec<ActiveMcpTool> {
    let mut resolved = Vec::new();
    for s in servers {
        if s.is_enabled != 1 {
            continue;
        }
        let config: McpServerConfig = s.into();

        // 1. First attempt to load tools from `mcp_server_tools` table
        let mut loaded_from_db = false;
        if let Ok(db_tools) = crate::agent::repository::list_mcp_server_tools(&s.id).await
            && !db_tools.is_empty()
        {
            for tool in db_tools {
                    let namespaced = format!("mcp__{}", tool.name);
                    // Synthesize definition from markdown spec
                    let def = parse_markdown_to_tool_def(&tool.name, &tool.config);
                    resolved.push(ActiveMcpTool {
                        server_id: config.id.clone(),
                        server_name: config.name.clone(),
                        original_name: tool.name.clone(),
                        namespaced_name: namespaced,
                        definition: def,
                        server_config: config.clone(),
                        markdown_spec: Some(tool.config),
                    });
                }
                loaded_from_db = true;
            }

            // 2. Fallback to live JSON-RPC list_tools if DB had no tools
        if !loaded_from_db {
            let client = McpClient::new(config.clone());
            match client.list_tools().await {
                Ok(tools) => {
                    for t in tools {
                        let namespaced = format!("mcp__{}", t.name);
                        resolved.push(ActiveMcpTool {
                            server_id: config.id.clone(),
                            server_name: config.name.clone(),
                            original_name: t.name.clone(),
                            namespaced_name: namespaced,
                            definition: t,
                            server_config: config.clone(),
                            markdown_spec: None,
                        });
                    }
                }
                Err(e) => {
                    crate::term_err!("[MCP] Failed to list tools from '{}': {}", s.name, e);
                }
            }
        }
    }
    resolved
}

/// Helper to parse tool parameters schema from Markdown contract or fallback
fn parse_markdown_to_tool_def(name: &str, config: &str) -> McpToolDefinition {
    let mut schema = serde_json::json!({
        "type": "object",
        "properties": {},
        "required": []
    });

    // Check for execution contract JSON inside markdown code fence
    let mut in_json = false;
    let mut json_buf = String::new();
    let mut desc_lines = Vec::new();

    for line in config.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```json") {
            in_json = true;
            continue;
        } else if in_json && trimmed.starts_with("```") {
            in_json = false;
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json_buf)
                && let Some(params) = v.get("parameters").or_else(|| v.get("inputSchema"))
            {
                schema = params.clone();
            }
            continue;
        }

        if in_json {
            json_buf.push_str(line);
            json_buf.push('\n');
        } else if !trimmed.starts_with('#') && !trimmed.is_empty() && desc_lines.len() < 3 {
            desc_lines.push(trimmed);
        }
    }

    let description = if !desc_lines.is_empty() {
        desc_lines.join(" ")
    } else {
        format!("Tool {} from MCP server", name)
    };

    McpToolDefinition {
        name: name.to_string(),
        description: Some(description),
        input_schema: schema,
    }
}

/// Dispatches a tool call to the matching active MCP server
pub async fn call_active_mcp_tool(
    tools: &[ActiveMcpTool],
    called_name: &str,
    arguments: serde_json::Value,
) -> Result<String, String> {
    let matched = tools
        .iter()
        .find(|t| t.namespaced_name == called_name || t.original_name == called_name)
        .ok_or_else(|| {
            format!(
                "MCP tool '{}' not found in active project tools",
                called_name
            )
        })?;

    let client = McpClient::new(matched.server_config.clone());
    let res = client.call_tool(&matched.original_name, arguments).await?;

    if res.is_error {
        let err_texts: Vec<String> = res
            .content
            .iter()
            .map(|c| {
                if let Some(t) = c.get("text").and_then(|v| v.as_str()) {
                    t.to_string()
                } else {
                    c.to_string()
                }
            })
            .collect();
        return Err(format!("MCP tool error: {}", err_texts.join("\n")));
    }

    let mut output = String::new();
    for item in res.content {
        if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
            output.push_str(text);
        } else if let Some(s) = item.as_str() {
            output.push_str(s);
        } else {
            output.push_str(&item.to_string());
        }
        output.push('\n');
    }

    Ok(output.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_markdown_to_tool_def_basic() {
        let md = r#"# search_files
Search files across the repository.

## Parameters
- `path` (string, required): Directory path.
- `pattern` (string, optional): Search regex.

## Execution Contract
```json
{
  "method": "tools/call",
  "name": "search_files",
  "parameters": {
    "type": "object",
    "properties": {
      "path": { "type": "string" },
      "pattern": { "type": "string" }
    },
    "required": ["path"]
  }
}
```
"#;
        let def = parse_markdown_to_tool_def("search_files", md);
        assert_eq!(def.name, "search_files");
        assert!(def.description.unwrap().contains("Search files across"));
        assert_eq!(def.input_schema["type"], "object");
        assert!(def.input_schema["properties"]["path"].is_object());
        assert_eq!(def.input_schema["required"][0], "path");
    }

    #[test]
    fn test_parse_markdown_to_tool_def_fallback() {
        let md = "# no_json_contract\nA simple tool without code block.";
        let def = parse_markdown_to_tool_def("no_json_contract", md);
        assert_eq!(def.name, "no_json_contract");
        assert!(def.description.unwrap().contains("A simple tool"));
        assert_eq!(def.input_schema["type"], "object");
    }
}