# ragasukma-core ⚡

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2021%2B-orange.svg)](https://www.rust-lang.org/)
[![Status](https://img.shields.io/badge/Status-Active-brightgreen.svg)]()

> **Headless Autonomous Agent Runtime, Tool Registry & Multi-Provider LLM Engine in Rust.**

`ragasukma-core` adalah inti kecerdasan (*headless brain*) mandiri yang dirancang murni untuk eksekusi **LLM** dan orkestrasi **Agent**. Dibuat bebas dari ketergantungan grafis (GPU/UI/VRM) maupun driver audio (TTS/STT), sehingga sangat ringan, cepat, dan portabel untuk disematkan ke berbagai consumer interface (seperti TUI berbasis terminal, CLI, server backend, atau desktop voice assistant).

---

## 🌟 Key Features

- **Multi-Provider LLM Engine**:
  - Dukungan streaming, tool call execution, token tracking, dan quota live adapter.
  - Multi-provider terpadu: **Antigravity (Google Gemini)**, **Claude (Anthropic)**, **Codex (OpenAI)**, **Qwen**, **Ollama**, dan provider kustom dengan standardisasi OpenAI-compatible.
  - Thought signature caching & sanitasi output AI terintegrasi (`cleaner`).

- **Autonomous Agent Runtime**:
  - Loop eksekusi otonom (turn-based reasoning, planning, state snapshot, dan rollback).
  - Context compression & token budget manager untuk sesi kerja jangka panjang.
  - Model Context Protocol (MCP) client dan Dynamic Skills runner.

- **Built-in Standard Tools**:
  - File System explorer & reader (`fs`).
  - Code search & grep tool (`search`).
  - Surgical code editor with context validation (`editor`).
  - Dialog / confirmation prompt flow (`dialogs`).

- **Zero Graphic / Zero Audio Bloat**:
  - **100% headless** tanpa dependensi `wgpu`, `winit`, `gltf`, `rodio`, `cpal`, atau ONNX runtime media.
  - Siap diintegrasikan ke TUI (`ratatui`), Web service (`axum`), atau CLI tool.

- **Embedded Telemetry & Persistence**:
  - SQLite telemetry engine menggunakan `sqlx` (riwayat percakapan, metrik token harian, dan audit tools).

---

## 📦 Installation

Tambahkan dependensi ke `Cargo.toml` proyek Anda:

```toml
[dependencies]
ragasukma-core = { git = "https://github.com/ahlanefwolf/ragasukma-core.git" }
```

Atau gunakan path jika berada dalam workspace lokal:

```toml
[dependencies]
ragasukma-core = { path = "../ragasukma-core" }
```

---

## 🚀 Quick Start

### 1. Inisialisasi LLM Client

```rust
use ragasukma_core::llm::{LlmClient, LlmConfig, ProviderType};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = LlmClient::new(LlmConfig {
        provider: ProviderType::Codex,
        model: "gpt-4o".to_string(),
        api_key: std::env::var("OPENAI_API_KEY").ok(),
        ..Default::default()
    });

    // Panggil model atau stream response
    Ok(())
}
```

### 2. Menjalankan Autonomous Agent Executor

```rust
use ragasukma_core::agent::executor::AgentExecutor;
use ragasukma_core::agent::tools::ToolRegistry;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let tools = ToolRegistry::default_all();
    let executor = AgentExecutor::new(tools);

    // Jalankan sesi otonom
    Ok(())
}
```

---

## 🧪 Testing

Jalankan rangkaian unit & integration tests:

```bash
cargo test
```

---

## 📄 License

Proyek ini dilisensikan di bawah **MIT License**. Lihat file [LICENSE](LICENSE) untuk detail lengkap.
