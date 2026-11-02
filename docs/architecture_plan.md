# Ragasukma Core - Architectural Separation Plan

This document outlines the architectural strategy for decoupling the **LLM** and **Autonomous Agent** modules from `ai-voice` into an independent, reusable library crate: **`ragasukma-core`**.

---

## 1. Why This Is a Good Decision

1. **Headless & Reusable**:
   `ai-voice` currently blends autonomous AI agent logic with desktop application concerns (audio capture/playback, WebRTC, WebSocket, system tray, Webview/UI). Extracting `ragasukma-core` enables the agent and LLM engines to run in headless environments: CLI utilities, background service daemons, backend APIs, or desktop interfaces.

2. **Faster Builds & Isolated CI**:
   Unit and integration tests for LLM providers, tool execution, context compression, scoring, and MCP clients can run in isolation within `ragasukma-core` without compiling heavy desktop multimedia and windowing dependencies (`cpal`, `rodio`, `webrtc`, `tao`, `wry`, etc.).

3. **Clear Boundary & Clean Architecture**:
   Eliminates spaghetti coupling between the GUI/data persistence layer and the autonomous agent loop. The core operates purely on domain contracts, traits, and event streams.

---

## 2. Module Mapping & Responsibilities

| Module | New Location in `ragasukma-core` | Notes & Strategy |
|---|---|---|
| **`llm/`** | `ragasukma-core::llm` | **100% migrated**: providers (OpenAI, Anthropic, Gemini, Groq, etc.), unified client, token usage tracker, prompt cleaners. Pure network/text I/O, zero GUI dependencies. |
| **`agent/context_builder`**, **`compressor`**, **`events`**, **`models`** | `ragasukma-core::agent` | **100% migrated**: token budgeting, system prompt formulation, context compaction, state machine types. |
| **`agent/tools`** & **`mcp`** | `ragasukma-core::tools` & `::mcp` | File manipulation tools (`read`, `write`, `edit`, `grep`), process execution runners, and Model Context Protocol (MCP) clients. |
| **`agent/tools/dialogs.rs`** | Interface / Trait Boundary | OS GUI dialogs (e.g., zenity, PowerShell, osascript) abstracted behind a `DialogProvider` trait or callback hook to keep core headless. |
| **`agent/service.rs`** / **WebSocket Events** | Event Bus Abstraction | Event dispatching to frontend consumers decoupled via `tokio::sync::broadcast` channels or an `AgentEventListener` trait. |

---

## 3. Incremental Execution Roadmap

### Phase 1: Initialize `ragasukma-core`
- Scaffold the library crate in `ragasukma-core/Cargo.toml` (`[package]`, `[lib]`).
- Configure baseline dependencies (`tokio`, `serde`, `serde_json`, `reqwest`, `async-trait`, `anyhow`, etc.).
- Connect `ai-voice` to `ragasukma-core` via path dependency:
  ```toml
  ragasukma-core = { path = "../rust/ragasukma-core" }
  ```

### Phase 2: Migrate `llm/` Module
- Move source files from `ai-voice/src/llm/` to `ragasukma-core/src/llm/`.
- Re-export in `ai-voice`:
  ```rust
  pub use ragasukma_core::llm;
  ```
  This ensures zero breaking changes across all existing consumers inside `ai-voice`.
- Run cargo checks, lints, and unit tests to ensure stability.

### Phase 3: Migrate `agent/` Module
- Relocate models, context builder, compressor, scoring heuristics, skills loader, and filesystem tools.
- Implement trait boundaries for OS dialogs and frontend event telemetry.
- Migrate the agent execution loop and orchestrator.
- Perform end-to-end integration and regression testing across both crates.
