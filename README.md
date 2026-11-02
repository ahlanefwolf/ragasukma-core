# ragasukma-core ⚡

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust Edition: 2024](https://img.shields.io/badge/Rust-2024-orange.svg)](https://www.rust-lang.org/)
[![Status](https://img.shields.io/badge/Status-Active-brightgreen.svg)]()

> **Headless Autonomous Agent Runtime, Tool Engine & Multi-Provider LLM Suite in Rust.**

`ragasukma-core` is a standalone, headless Rust library that powers the **Multi-Provider LLM Engine**, **Tool Registry**, and **Autonomous Agent Orchestrator** for Ragasukma. Built strictly without graphical (`wgpu`/`winit`) or audio driver (`rodio`/`cpal`) dependencies, it is lightweight, highly portable, and ready to be embedded into any runtime environment—such as TUIs (terminal interfaces), CLI utilities, daemon services, background workers, or voice assistant backends.

---

## 🌟 Architecture & Core Features

### 1. Multi-Provider LLM Engine (`ragasukma_core::llm`)
- **OpenAI-Compatible V1 Engine** (`ragasukma_core::llm::v1_engine`):
  - Token-by-token streaming response handling.
  - Native structured tool calling and function call execution.
  - Flexible authentication (Bearer tokens, custom headers, and per-request model overrides).
- **Multi-Provider Hub** (`ragasukma_core::llm::providers`):
  - **Antigravity (Google Gemini)**: OAuth flow, real-time quota tracking, and thought signature support.
  - **Claude (Anthropic)**: Native message stream adapter and adaptive token budgeting.
  - **Codex (OpenAI)**: Direct client integration with dynamic model discovery.
  - **Custom / Local Endpoints**: Ollama, vLLM, LMStudio, Qwen, DeepSeek, and custom OpenAI-compatible proxies.
- **SQLite Usage & Token Telemetry** (`ragasukma_core::llm::usage`):
  - Tracks prompt tokens, completion tokens, execution latency, cost approximations, and auto-reaps inactive database locks.

### 2. SQLite Database & Repositories (`ragasukma_core::repository`)
- **Automated Database Migrations** (`repository::migrate_database`):
  - Embeds and executes SQLx migrations automatically for schema initialization and version control.
- **Data Repositories**:
  - **Chat History** (`repository::chat_history`): Fast persistence and retrieval of historical conversation turns.
  - **Skills** (`repository::skill`): Storage, execution status, and management of user-defined capabilities.
  - **Todos** (`repository::todo`): Task management and lifecycle tracking.

### 3. Autonomous Agent Engine (`ragasukma_core::agent`)
- **Turn-Based Agent Execution** (`agent::executor::execute_session_turn`):
  - Autonomous reasoning loop, safety gates, dynamic iteration limits, and graceful fallback recovery.
  - Context-aware dynamic system prompt generation (`agent::executor::prompt`).
- **Context Compressor & Token Budget Manager** (`agent::compressor`):
  - Smart historical conversation truncation (`compact_assistant_tool_calls_smart`, `truncate_smart`) to maximize context retention within token limits.
- **Evidence Cache & Ranking** (`agent::evidence_ranking`, `agent::evidence_scoring`):
  - Recency- and relevance-weighted ordering for files, grep matches, and diagnostic logs within the active context window.
- **Rollback & State Snapshot Store** (`agent::executor::snapshot`):
  - Automatic pre-modification file snapshots allowing complete rollback upon revert instructions.

### 4. Built-in Tools & Extensibility (`ragasukma_core::agent::tools`)
Standard workspace tools executable autonomously by the agent runtime:
- **`workspace_run_command`**: Executes shell commands on the host OS with strict safety checks, sandboxing guards, and timeouts.
- **`workspace_read_file`**: Reads files with line slicing (`start_line`, `end_line`) or structural symbol outline generation.
- **`workspace_edit_file`**: Precision line/block replacement with automatic rollback on error.
- **`workspace_write_file`**: Creates or completely overwrites files with built-in language formatters.
- **`workspace_list_dir`**: Shallow or depth-bounded directory explorer with item counts.
- **`workspace_grep`**: Fast regex and literal text searching across project trees.
- **MCP Client (`agent::mcp`)**: Extensible integration with external Model Context Protocol servers.

---

## 📦 Installation

Add `ragasukma-core` to your `Cargo.toml`:

```toml
[dependencies]
ragasukma-core = { git = "https://github.com/ahlanefwolf/ragasukma-core.git" }
tokio = { version = "1", features = ["full"] }
```

Or using a local path reference:

```toml
[dependencies]
ragasukma-core = { path = "../ragasukma-core" }
```

---

## 🚀 Quick Start & Usage

### 1. Direct LLM Chat Completion (V1 Engine)

```rust
use ragasukma_core::llm::v1_engine::{execute_chat_v1, V1ChatRequest, V1ChatMessage};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let req = V1ChatRequest {
        model: "gpt-4o".to_string(),
        messages: vec![
            V1ChatMessage {
                role: "system".to_string(),
                content: Some("You are a helpful assistant.".into()),
                ..Default::default()
            },
            V1ChatMessage {
                role: "user".to_string(),
                content: Some("Explain Rust ownership in one sentence.".into()),
                ..Default::default()
            },
        ],
        stream: Some(false),
        temperature: Some(0.7),
        ..Default::default()
    };

    let base_url = "https://api.openai.com/v1";
    let api_key = std::env::var("OPENAI_API_KEY")?;

    let response = execute_chat_v1(base_url, &api_key, &req).await?;
    println!("Response: {}", response);
    Ok(())
}
```

### 2. Executing an Autonomous Agent Turn

```rust
use ragasukma_core::agent::executor::execute_session_turn;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let session_id = "sess_demo_123";
    // Runs an autonomous reasoning, tool execution, and response cycle
    let result = execute_session_turn(session_id, None).await?;
    println!("Agent Output: {}", result);
    Ok(())
}
```

### 3. Database Migrations & Repositories

```rust
use sqlx::sqlite::SqlitePoolOptions;
use ragasukma_core::repository::{migrate_database, get_skills};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let pool = SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await?;

    // Run embedded SQLx schema migrations automatically
    migrate_database(&pool).await?;

    // Query skills
    let skills = get_skills(&pool).await?;
    println!("Loaded {} skill(s)", skills.len());
    Ok(())
}
```

---

## 🧪 Testing

Run the test suite across unit and integration tests:

```bash
cargo test
```

---

## 📄 License

Distributed under the **MIT License**. See the [LICENSE](LICENSE) file for details.
