# Rancangan Arsitektur MCP Ragasukma (Dynamic Session & In-Memory Registry)

Dokumen ini merangkum keputusan arsitektur untuk perombakan sistem integrasi **Model Context Protocol (MCP)** pada Ragasukma, beralih dari skema penyimpanan tools statis di SQLite menjadi **Dynamic On-Demand Session Lifecycle dengan In-Memory Tool Registry**.

---

## 1. Latar Belakang & Masalah Sebelumnya

1. **Stale Schema & Redundant Data di Database:**
   - Menyimpan daftar tools ke tabel SQLite (`mcp_server_tools`) menimbulkan risiko skema basi (*stale schema*) ketika server MCP upstream menambahkan atau mengubah parameter tool.
   - Terjadi inkonsistensi format (markdown vs CLI command flags vs raw JSON-RPC).
2. **Kekakuan Junction Project:**
   - Tabel `project_mcp_servers` menambah kompleksitas kueri dan mengharuskan konfigurasi ulang manual saat berpindah project, padahal MCP server umumnya bersifat kapabilitas global sistem (misal: Text-to-Speech Cartesia, Web Browser Playwright).
3. **Konflik Kredensial Antar Sesi:**
   - Konfigurasi `env` (seperti API key atau token autentikasi) yang disimpan global memicu *token collision* jika user ingin menggunakan akun/kunci API berbeda di sesi chat berbeda.

---

## 2. Keputusan Desain & Perubahan Skema Database

### A. Tabel yang Dihapus (Deprecate & Drop)
- ❌ **`mcp_server_tools`**: Dihapus total. Tools tidak lagi disimpan di database, melainkan dikelola langsung di **RAM**.
- ❌ **`project_mcp_servers`**: Dihapus total. Scope MCP beralih dari level project ke level **Sesi Chat** yang dikendalikan dinamis oleh Agent LLM.

### B. Tabel Baru & Skema Final

#### 1. Tabel `mcp_servers` (Katalog Blueprint Global)
Menyimpan definisi dasar server MCP yang terdaftar di sistem.

```sql
CREATE TABLE mcp_servers (
    id TEXT PRIMARY KEY,               -- e.g. 'cartesia', 'playwright', 'figcraft'
    type TEXT NOT NULL,                -- 'stdio' | 'http'
    config TEXT NOT NULL,              -- JSON string konfigurasi dasar (url, command, default args)
    handshake INTEGER NOT NULL DEFAULT 1, -- 1: wajib handshake initialize & session-id, 0: direct
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
```

**Contoh isi kolom `config`:**
- **HTTP / SSE (`type = 'http'`):**
  ```json
  {
    "url": "https://mcp.cartesia.ai/mcp"
  }
  ```
- **Stdio Local Process (`type = 'stdio'`):**
  ```json
  {
    "command": "figcraft-cli",
    "args": ["mcp"]
  }
  ```

---

#### 2. Tabel `session_mcp_servers` (State & Kredensial Per-Sesi)
Menyimpan server apa saja yang aktif pada sesi chat tertentu beserta variabel rahasia (`env`) khusus sesi tersebut.

```sql
CREATE TABLE session_mcp_servers (
    session_id TEXT NOT NULL,          -- ID sesi chat aktif
    server_id TEXT NOT NULL,           -- Foreign key ke mcp_servers(id)
    env TEXT NOT NULL DEFAULT '{}',    -- JSON string key-value rahasia/token sesi (e.g. {"CARTESIA_API_KEY": "sk_car_xxx"})
    is_active INTEGER NOT NULL DEFAULT 1, -- 1: aktif di sesi ini, 0: dinonaktifkan sementara
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (session_id, server_id),
    FOREIGN KEY (server_id) REFERENCES mcp_servers(id) ON DELETE CASCADE
);
```

---

## 3. In-Memory Tool Registry (RAM-Based)

Tidak ada lagi konversi markdown atau string parsing untuk tools di SQLite. Seluruh daftar tool disimpan di memori selama sesi berjalan:

```rust
// Representasi di Rust Agent Engine
pub struct SessionMcpRegistry {
    // Dipetakan per session_id -> server_id -> list of tool definitions
    pub active_tools: Arc<RwLock<HashMap<String, Vec<McpToolDefinition>>>>,
    pub http_sessions: Arc<RwLock<HashMap<String, String>>>, // server_id -> mcp-session-id
}
```

### Keuntungan In-Memory Registry:
1. **Always Fresh:** Saat session dibuka atau server dihubungkan, Agent menjalankan handshake `initialize` $\rightarrow$ `tools/list`. Tool schema dijamin 100% selalu paling mutakhir.
2. **Native JSON Schema:** Langsung diteruskan ke LLM Function Calling tanpa kompromi parsing CLI flags atau escaping kutip di shell.
3. **Zero Waste:** Saat sesi ditutup atau server dinonaktifkan, RAM dibersihkan tanpa meninggalkan sampah record di database.

---

## 4. Otonomi Agent via Chat Session (Dynamic LLM Control)

Agent diberikan kapabilitas/tool internal untuk memanipulasi MCP secara on-demand selama percakapan:

1. **Discovery / Register Server:**
   - User: *"Hubungkan ke Cartesia TTS, endpointnya https://mcp.cartesia.ai/mcp"*
   - LLM: Mengidentifikasi tipe server (`http`), mengecek handshake (`1`), lalu membuat entry di `mcp_servers`.
2. **Per-Session Credential Injection:**
   - User: *"Ini API key saya: `sk_car_...`"*
   - LLM: Menyimpan key ke `session_mcp_servers.env` untuk sesi chat tersebut saja. Sesi lain tetap aman dan terisolasi.
3. **Session Lifecycle Toggle:**
   - User: *"Matikan tool suara, sekarang fokus ngoding saja"*
   - LLM: Mengubah `session_mcp_servers.is_active = 0` $\rightarrow$ Registry di RAM langsung mendepak tool audio agar tidak mengotori konteks prompt LLM.
