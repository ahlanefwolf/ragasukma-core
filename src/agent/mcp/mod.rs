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

/// Split a shell command string into separate words, respecting quotes and escapes.
fn split_shell_words(s: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_quotes = None;
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(next_c) = chars.next() {
                    current.push(next_c);
                }
            }
            '"' | '\'' => {
                if in_quotes == Some(c) {
                    in_quotes = None;
                } else if in_quotes.is_none() {
                    in_quotes = Some(c);
                } else {
                    current.push(c);
                }
            }
            ' ' | '\t' if in_quotes.is_none() => {
                if !current.is_empty() {
                    words.push(current);
                    current = String::new();
                }
            }
            _ => current.push(c),
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

/// Resolves executable command and arguments directly via system PATH.
fn resolve_executable(
    command_str: &str,
    args: &[String],
) -> (String, Vec<String>, Option<std::path::PathBuf>) {
    let trimmed = command_str
        .trim()
        .trim_matches('`')
        .trim_matches('"')
        .trim_matches('\'');
    let words = split_shell_words(trimmed);
    if words.is_empty() {
        return (command_str.to_string(), args.to_vec(), None);
    }
    let binary = words[0].clone();
    let mut final_args = words[1..].to_vec();
    final_args.extend_from_slice(args);
    (binary, final_args, None)
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
            is_enabled: s.is_enabled,
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
        if !s.is_enabled {
            continue;
        }
        let config: McpServerConfig = s.into();

        // 1. First attempt to load tools from `mcp_server_tools` table
        let mut loaded_from_db = false;
        if let Ok(db_tools) = crate::repository::agent::list_mcp_server_tools(&s.id).await
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

        // 2. If no tools in DB, attempt on-demand tool discovery & persistence from server configuration
        if !loaded_from_db {
            match discovery::sync_server_tools(s, None).await {
                Ok(discovered) if !discovered.is_empty() => {
                    for tool in discovered {
                        let namespaced = format!("mcp__{}", tool.name);
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
                Ok(_) => {}
                Err(e) => {
                    crate::term_err!("[MCP] On-demand tool sync failed for '{}': {}", s.name, e);
                }
            }
        }

        // 3. Fallback: query tools dynamically via live JSON-RPC handshake
        if !loaded_from_db {
            let client = McpClient::new(config.clone());
            match client.list_tools().await {
                Ok(tools) => {
                    for t in tools {
                        let namespaced = format!("mcp__{}", t.name);
                        let md = discovery::tool_definition_to_markdown(&t);
                        resolved.push(ActiveMcpTool {
                            server_id: config.id.clone(),
                            server_name: config.name.clone(),
                            original_name: t.name.clone(),
                            namespaced_name: namespaced,
                            definition: t,
                            server_config: config.clone(),
                            markdown_spec: Some(md.config),
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

/// Helper to parse tool parameters schema directly from Markdown specification
fn parse_markdown_to_tool_def(name: &str, config: &str) -> McpToolDefinition {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    let mut desc_lines = Vec::new();
    let mut in_parameters = false;

    for line in config.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with('#') {
            let lower = trimmed.to_lowercase();
            in_parameters = lower.contains("parameter");
            continue;
        }

        if in_parameters {
            // Parses format: - `param_name` (type, required/optional): description
            // Also handles: - param_name: (type, required) description
            if let Some(bullet) = trimmed
                .strip_prefix('-')
                .or_else(|| trimmed.strip_prefix('*'))
            {
                let bullet_trimmed = bullet.trim();
                let (first_part, desc_part) = bullet_trimmed
                    .split_once(':')
                    .unwrap_or((bullet_trimmed, ""));

                // Extract param name
                let param_name = if let Some(start) = first_part.find('`') {
                    if let Some(end) = first_part[start + 1..].find('`') {
                        &first_part[start + 1..start + 1 + end]
                    } else {
                        first_part.trim()
                    }
                } else {
                    first_part.split_whitespace().next().unwrap_or("").trim()
                };

                let clean_param =
                    param_name.trim_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '-');

                if !clean_param.is_empty() {
                    // Look for type & requirement across entire bullet line for robustness
                    let combined_meta = format!("{} {}", first_part, desc_part).to_lowercase();
                    let is_req =
                        combined_meta.contains("required") && !combined_meta.contains("optional");

                    let ptype = if combined_meta.contains("number")
                        || combined_meta.contains("integer")
                        || combined_meta.contains("int")
                        || combined_meta.contains("float")
                    {
                        "number"
                    } else if combined_meta.contains("bool") {
                        "boolean"
                    } else if combined_meta.contains("array") || combined_meta.contains("list") {
                        "array"
                    } else if combined_meta.contains("object")
                        || combined_meta.contains("map")
                        || combined_meta.contains("json")
                    {
                        "object"
                    } else {
                        "string"
                    };

                    let mut prop_obj = serde_json::Map::new();
                    prop_obj.insert(
                        "type".to_string(),
                        serde_json::Value::String(ptype.to_string()),
                    );

                    // Clean description
                    let clean_desc = desc_part.trim().trim_start_matches(|c: char| {
                        c == '(' || c == ')' || c == ',' || c.is_whitespace()
                    });
                    if !clean_desc.is_empty() {
                        prop_obj.insert(
                            "description".to_string(),
                            serde_json::Value::String(clean_desc.to_string()),
                        );
                    }

                    properties.insert(clean_param.to_string(), serde_json::Value::Object(prop_obj));
                    if is_req {
                        required.push(serde_json::Value::String(clean_param.to_string()));
                    }
                }
            }
        } else if !trimmed.starts_with('#') && !trimmed.is_empty() && desc_lines.len() < 3 {
            desc_lines.push(trimmed);
        }
    }

    let description = if !desc_lines.is_empty() {
        desc_lines.join(" ")
    } else {
        format!("Tool {} from MCP server", name)
    };

    let mut schema_map = serde_json::Map::new();
    schema_map.insert(
        "type".to_string(),
        serde_json::Value::String("object".to_string()),
    );
    schema_map.insert(
        "properties".to_string(),
        serde_json::Value::Object(properties),
    );
    schema_map.insert("required".to_string(), serde_json::Value::Array(required));

    McpToolDefinition {
        name: name.to_string(),
        description: Some(description),
        input_schema: serde_json::Value::Object(schema_map),
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

    // 1. Skill paradigm: if the tool's markdown spec carries a Command Template
    //    (generated by the LLM in Tahap 1), execute it directly. Arguments from the
    //    LLM are substituted into `{param}` placeholders. The LLM stays in control:
    //    it chooses the arguments each call and may improvise its own commands via
    //    workspace_run_command at any time (Tahap 2 is never locked to a template).
    if let Some(spec) = &matched.markdown_spec
        && let Some(template) = extract_command_template(spec)
    {
        return run_command_template(&template, &arguments).await;
    }

    // 2. Fallback: native MCP JSON-RPC tools/call
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

/// Extracts the command template from either a raw CLI command string or a
/// markdown spec's `## Command Template` block.
pub fn extract_command_template(spec: &str) -> Option<String> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return None;
    }

    let lower = trimmed.to_ascii_lowercase();
    let marker = "## command template";
    if let Some(idx) = lower.find(marker) {
        let rest = &trimmed[idx + marker.len()..];
        if let Some(fence_start) = rest.find("```") {
            let after_fence = &rest[fence_start + 3..];
            let after_lang = if let Some(nl) = after_fence.find('\n') {
                let first_line = after_fence[..nl].trim();
                if first_line.is_empty() || first_line.split_whitespace().count() <= 2 {
                    &after_fence[nl + 1..]
                } else {
                    after_fence
                }
            } else {
                after_fence
            };
            if let Some(end) = after_lang.find("```") {
                let template = after_lang[..end].trim().to_string();
                if !template.is_empty() {
                    return Some(template);
                }
            }
        }
    }

    // If it's wrapped in a code fence ```...```, unwrap it
    if trimmed.starts_with("```") && trimmed.ends_with("```") {
        let inner = trimmed[3..trimmed.len() - 3].trim();
        let stripped = if let Some(nl) = inner.find('\n') {
            let first_line = inner[..nl].trim();
            if first_line.is_empty() || first_line.split_whitespace().count() <= 2 {
                &inner[nl + 1..]
            } else {
                inner
            }
        } else {
            inner
        };
        let cleaned = stripped.trim().to_string();
        if !cleaned.is_empty() {
            return Some(cleaned);
        }
    }

    // If it's a raw command string without markdown headings, return it directly
    if !trimmed.contains("# ") {
        return Some(trimmed.to_string());
    }

    None
}

/// Safely escapes a string value to be enclosed within double-quotes in a shell command.
/// Neutralizes shell injection vectors such as $(...), `...`, quotes, variable expansions,
/// and command separators.
pub fn escape_shell_arg_value(val: &str) -> String {
    let mut escaped = String::with_capacity(val.len() + 16);
    for c in val.chars() {
        match c {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '$' => escaped.push_str("\\$"),
            '`' => escaped.push_str("\\`"),
            '!' => escaped.push_str("\\!"),
            '\n' => escaped.push(' '),
            '\r' => {}
            other => escaped.push(other),
        }
    }
    escaped
}

/// Replaces `{param}` placeholders in a command template with escaped LLM argument
/// values. Also cleans up optional CLI flags (e.g. `--flag "{param}"`) when the argument
/// is missing, null, or empty so the CLI does not fail on empty flag values.
pub fn render_template_placeholders(template: &str, arguments: &serde_json::Value) -> String {
    let mut rendered = template.to_string();
    let empty_map = serde_json::Map::new();
    let args_obj = arguments.as_object().unwrap_or(&empty_map);

    // Find all {param} in the template
    let mut placeholders = Vec::new();
    let mut i = 0;
    while let Some(start) = rendered[i..].find('{') {
        let abs_start = i + start;
        if let Some(end) = rendered[abs_start..].find('}') {
            let abs_end = abs_start + end;
            let param_name = &rendered[abs_start + 1..abs_end];
            if !param_name.is_empty() && !placeholders.contains(&param_name.to_string()) {
                placeholders.push(param_name.to_string());
            }
            i = abs_end + 1;
        } else {
            break;
        }
    }

    for key in placeholders {
        let placeholder = format!("{{{}}}", key);
        let val_opt = args_obj.get(&key);
        let val_str = match val_opt {
            Some(serde_json::Value::String(s)) if !s.trim().is_empty() => Some(s.clone()),
            Some(serde_json::Value::Number(n)) => Some(n.to_string()),
            Some(serde_json::Value::Bool(b)) => Some(b.to_string()),
            _ => None,
        };

        if let Some(val) = val_str {
            let escaped = escape_shell_arg_value(&val);
            rendered = rendered.replace(&placeholder, &escaped);
        } else {
            // Optional parameter was not supplied or empty.
            // Check if it was preceded by an optional flag like: --flag "{param}" or -f "{param}"
            let patterns_to_check = [
                format!("\"{}\"", placeholder),
                format!("'{}'", placeholder),
                placeholder.clone(),
            ];

            let mut removed_flag = false;
            for p in &patterns_to_check {
                if let Some(p_idx) = rendered.find(p.as_str()) {
                    let before = &rendered[..p_idx];
                    let trimmed_before = before.trim_end();
                    if let Some(flag_start) = trimmed_before.rfind(|c: char| c.is_whitespace()) {
                        let candidate_flag = trimmed_before[flag_start + 1..].trim();
                        if candidate_flag.starts_with('-') {
                            // Erase from flag_start to p_idx + p.len()
                            let slice_to_remove = &rendered[flag_start..p_idx + p.len()];
                            rendered = rendered.replacen(slice_to_remove, "", 1);
                            removed_flag = true;
                            break;
                        }
                    }
                }
            }

            if !removed_flag {
                // Simply replace placeholder with empty string
                rendered = rendered.replace(&placeholder, "");
            }
        }
    }

    rendered
}

/// OS safety-net: rewrites common bash idioms into PowerShell-compatible syntax so
/// LLM-generated templates run on Windows without "curl bash syntax" failures.
/// Pure string transform, unit-testable on every OS.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn normalize_command_for_windows(cmd: &str) -> String {
    cmd.replace("curl ", "curl.exe ")
        .replace("python3", "python")
        .replace("&&", ";")
        .replace("'", "\"")
}

/// Maximum allowed output bytes from an MCP CLI tool to avoid exhausting context window.
pub const MCP_CLI_MAX_OUTPUT_BYTES: usize = 64 * 1024;
/// Maximum timeout allowed for an MCP tool command.
pub const MCP_CLI_TIMEOUT_SECONDS: u64 = 45;

/// Renders `{param}` placeholders from the LLM arguments and executes the command
/// template via the system shell with timeout protection and output capping.
pub async fn run_command_template(
    template: &str,
    arguments: &serde_json::Value,
) -> Result<String, String> {
    let rendered = render_template_placeholders(template, arguments);

    // OS safety-net: LLM-generated templates are often written in bash idiom even
    // on Windows. Normalize the most common traps BEFORE execution so "curl bash
    // syntax in PowerShell" errors never reach the user.
    #[cfg(windows)]
    let rendered = normalize_command_for_windows(&rendered);

    // Windows: use PowerShell; Unix: use sh -c
    #[cfg(windows)]
    let mut shell_command = {
        let mut c = Command::new("powershell");
        c.args(["-NoProfile", "-Command", "-"]);
        c.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        c
    };
    #[cfg(not(windows))]
    let mut shell_command = {
        let mut c = Command::new("sh");
        c.arg("-c").arg(&rendered);
        c.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        c
    };

    #[cfg(windows)]
    let mut child = shell_command
        .spawn()
        .map_err(|e| format!("Failed to start shell: {}", e))?;
    #[cfg(not(windows))]
    let child = shell_command
        .spawn()
        .map_err(|e| format!("Failed to start shell: {}", e))?;

    #[cfg(windows)]
    {
        use tokio::io::AsyncWriteExt;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(rendered.as_bytes())
                .await
                .map_err(|e| format!("Failed to feed command: {}", e))?;
        }
    }

    let wait_future = child.wait_with_output();
    let output = match tokio::time::timeout(
        std::time::Duration::from_secs(MCP_CLI_TIMEOUT_SECONDS),
        wait_future,
    )
    .await
    {
        Ok(res) => res.map_err(|e| format!("Failed to run command: {}", e))?,
        Err(_) => {
            return Err(format!(
                "Command timed out after {} seconds: {}",
                MCP_CLI_TIMEOUT_SECONDS, rendered
            ));
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut combined = String::new();
    if !stdout.is_empty() {
        combined.push_str(&stdout);
    }
    if !stderr.is_empty() {
        if !combined.is_empty() {
            combined.push('\n');
        }
        combined.push_str(&stderr);
    }

    let mut result_str = combined.trim().to_string();

    // Output capping to protect LLM context window
    if result_str.len() > MCP_CLI_MAX_OUTPUT_BYTES {
        let head_bytes = MCP_CLI_MAX_OUTPUT_BYTES / 2;
        let tail_bytes = MCP_CLI_MAX_OUTPUT_BYTES / 4;
        let head = &result_str[..result_str.floor_char_boundary(head_bytes)];
        let tail_start =
            result_str.floor_char_boundary(result_str.len().saturating_sub(tail_bytes));
        let tail = &result_str[tail_start..];
        result_str = format!(
            "{}\n\n[... Truncated {} bytes of MCP output to protect context window ...]\n\n{}",
            head,
            result_str.len() - (head.len() + tail.len()),
            tail
        );
    }

    if output.status.success() {
        Ok(result_str)
    } else {
        Err(format!(
            "Command exited with {}: {}",
            output.status, result_str
        ))
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
