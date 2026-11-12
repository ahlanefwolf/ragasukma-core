# Rekomendasi Perbaikan Ragasukma

**Target:** `ragasukma-core` dan integrasinya dengan `ragasukma-tui`\
**Tanggal:** 12 November 2026\
**Jenis:** Rangkuman static source review; bukan hasil penetration test
atau benchmark.

## 1. Ringkasan eksekutif

Ragasukma memiliki fondasi yang menjanjikan: agent lifecycle, goal
orchestration, context management, evidence handling, tool execution,
provider streaming, session-aware snapshots, dan verification states.

Prioritas pengembangan sebaiknya bukan menambah fitur, melainkan
memperkuat correctness, keamanan filesystem, concurrency, cancellation,
dan pembuktian hasil kerja agent.

  -----------------------------------------------------------------------
  Prioritas               Area                    Rekomendasi
  ----------------------- ----------------------- -----------------------
  P0                      Filesystem security     Perkuat validasi path
                                                  dan symlink handling

  P0                      Session concurrency     Terapkan eksklusivitas
                                                  per session

  P0                      Stream cancellation     Pastikan timeout
                                                  benar-benar
                                                  menghentikan
                                                  request/worker

  P1                      Verification            Jadikan hasil
                                                  verifikasi berbasis
                                                  bukti

  P1                      Checkpoint              Tambahkan checkpoint
                                                  durable dan atomic file
                                                  replacement

  P1                      Tool scheduler          Paralelkan tool
                                                  independen secara
                                                  terkontrol

  P1                      Shell execution         Perjelas sandbox,
                                                  permission, timeout,
                                                  dan audit

  P2                      LLM abstraction         Konsolidasikan jalur
                                                  client sync/async

  P2                      Token accounting        Utamakan usage aktual
                                                  provider

  P2                      Testing                 Tambahkan integration,
                                                  concurrency, dan
                                                  fault-injection tests
  -----------------------------------------------------------------------

## 2. Kekuatan yang perlu dipertahankan

-   **Goal lifecycle:** state seperti `Discovery`, `Action`,
    `Verification`, `Completed`, `CompletedUnverified`, dan `Failed`
    membuat lifecycle eksplisit.
-   **Session-aware snapshots:** snapshot terpisah per session
    mengurangi risiko backup saling menghapus.
-   **Tool audit:** status executed, denied, dan failed, repeated-read
    detection, error signature, serta circuit breaker merupakan fondasi
    observability yang berguna.
-   **Streaming retry:** backoff, jitter, penanganan 429/5xx,
    `Retry-After`, dan stream reset adalah keputusan yang baik.
-   **Context/evidence subsystem:** context builder, compression,
    evidence ranking, dan scoring layak dipertahankan serta diukur.

Jangan melakukan rewrite besar yang menghilangkan mekanisme tersebut
sebelum tersedia pengganti dan regression tests.

## 3. P0 --- Perbaikan kritis

### 3.1 Perkuat validasi path workspace

**Risiko:** jalur untuk target baru dan parent yang belum ada berpotensi
mempertahankan path non-canonical. Validasi `starts_with()` terhadap
path yang belum dinormalisasi tidak cukup sebagai security boundary.
Komponen `..` dan symlink dapat mengarah ke luar workspace.

Ini merupakan **potensi path traversal** yang harus dikonfirmasi dengan
regression test dan PoC terkontrol.

**Rekomendasi:** 1. Canonicalize dan simpan workspace root saat
workspace didaftarkan. 2. Tolak path absolut jika API hanya menerima
path relatif. 3. Normalisasi komponen `.` dan `..` secara eksplisit. 4.
Untuk target baru, cari ancestor terdekat yang sudah ada, canonicalize
ancestor tersebut, lalu validasi komponen relatif yang tersisa. 5.
Tetapkan kebijakan symlink: tolak symlink yang keluar dari root atau
ikuti hanya jika target akhirnya tetap berada di root. 6. Perhatikan
TOCTOU antara validasi dan operasi filesystem. 7. Untuk batas keamanan
yang kuat, pertimbangkan operasi relatif terhadap directory
handle/descriptor dan mekanisme OS yang membatasi symlink traversal.

**Regression tests minimum:** - `../outside/file` -
`../../outside/new-dir/file` - Target baru dengan parent yang belum
ada - Symlink di dalam workspace yang menunjuk ke luar - Symlink yang
berubah setelah validasi (race test bila memungkinkan) - Workspace
`/work/project` versus `/work/project-evil` - Absolute path dan
separator berbeda sesuai platform

**Kriteria penerimaan:** tidak ada operasi read, write, create, rename,
atau delete yang dapat menyentuh target di luar workspace yang
diizinkan.

### 3.2 Terapkan per-session execution lock

Generation counter membantu mencegah cleanup lama mengubah status turn
baru, tetapi tidak otomatis menjamin hanya satu turn berjalan pada
session yang sama.

**Risiko:** dua turn dapat membaca history yang sama, mengubah goal
state, atau memodifikasi file secara bersamaan.

**Rekomendasi:** - Tambahkan per-session async mutex atau actor/queue. -
Lock mencakup seluruh lifecycle: load state, context construction,
inference, tool execution, persistence, verification, dan
finalization. - Definisikan perilaku request kedua: menunggu, ditolak
sebagai `session_busy`, atau menggantikan turn lama melalui cancellation
eksplisit. - Hindari satu global mutex untuk semua session. - Bersihkan
registry lock tanpa menghapus lock yang masih digunakan waiter.

**Kriteria penerimaan:** satu session tidak menjalankan dua lifecycle
turn bersamaan; session berbeda tetap dapat paralel; cancellation satu
session tidak memengaruhi session lain.

### 3.3 Benahi cancellation streaming yang memakai `spawn_blocking`

Timeout terhadap future yang menunggu `JoinHandle` dari `spawn_blocking`
tidak otomatis menghentikan blocking task yang sudah berjalan.

**Risiko:** agent melaporkan timeout sementara worker lama masih hidup,
memakai resource, atau menghasilkan efek terlambat.

**Rekomendasi:** 1. Utamakan client HTTP async-native untuk streaming.
2. Berikan cancellation token per request/turn. 3. Teruskan cancellation
ke HTTP request/stream dan loop pemrosesan. 4. Pisahkan connect timeout,
read/idle timeout, dan total request timeout. 5. Jika blocking API tetap
digunakan, dokumentasikan keterbatasan pembatalannya dan gunakan
cooperative cancellation serta timeout transport. 6. Jangan memulai
retry sebelum attempt sebelumnya berhenti atau dipastikan tidak
menghasilkan efek. 7. Bedakan timeout, user cancellation, provider
disconnect, dan internal error.

**Kriteria penerimaan:** setelah timeout/cancellation, tidak ada worker
yang terus mengonsumsi resource atau mengirim event untuk turn yang
sudah berakhir.

## 4. P1 --- Keandalan agent runtime

### 4.1 Verification harus berbasis bukti

Permintaan menjalankan test bukan bukti bahwa test benar-benar
dijalankan. Flag seperti `verification_prompted` tidak boleh dianggap
sebagai hasil verifikasi.

Gunakan hasil terstruktur, misalnya:

``` rust
struct VerificationEvidence {
    verifier: String,
    command: Vec<String>,
    status: VerificationStatus,
    exit_code: Option<i32>,
    started_at: DateTime<Utc>,
    finished_at: DateTime<Utc>,
    output_ref: Option<String>,
}
```

Status minimal: `NotRun`, `Passed`, `Failed`, `Unavailable`, dan
`SkippedWithReason`.

Aturan penyelesaian: - `Completed`: verifier wajib lulus dan bukti
tersimpan. - `CompletedUnverified`: verifier tidak tersedia, dilewati
dengan alasan, atau tidak dapat dijalankan. - `Failed`: pekerjaan gagal
atau verifikasi wajib gagal setelah batas repair tercapai.

Jangan menyimpulkan keberhasilan hanya dari teks final LLM.

### 4.2 Buat tool scheduler berbasis side effect

Pemilihan satu tool call dari beberapa tool call mengurangi risiko
concurrency, tetapi menambah round-trip model.

  -----------------------------------------------------------------------
  Kategori                Contoh                  Kebijakan
  ----------------------- ----------------------- -----------------------
  Read-only               Read file, list         Paralel dengan batas
                          directory, grep         concurrency

  Mutating                Write/edit file         Serial per path

  Process                 Shell command, test     Serial secara default
                          runner                  

  Destructive             Delete, rollback, reset Exclusive dan
                                                  policy/approval

  External                MCP/network tools       Berdasarkan side-effect
                                                  dan permission
  -----------------------------------------------------------------------

Scheduler harus menghormati dependency, mencegah operasi mutating pada
resource sama, memiliki global/per-session concurrency limit,
mengembalikan hasil berdasarkan `tool_call_id`, mendukung cancellation,
dan mencatat urutan eksekusi.

Jangan menjalankan semua tool secara paralel hanya karena model mengirim
beberapa tool call.

### 4.3 Perkuat checkpoint dan file editing

Snapshot in-memory berguna selama proses hidup, tetapi tidak cukup untuk
recovery setelah crash.

**Rekomendasi:** - Tambahkan checkpoint durable melalui SQLite atau
journal file. - Simpan session, path, status, ukuran, hash, dan lokasi
backup. - Gunakan temporary file pada filesystem yang sama untuk
replacement. - Tulis konten, flush, `sync_all`, lalu rename secara
atomik jika platform mendukung. - Persist metadata checkpoint dengan
urutan konsisten terhadap operasi filesystem. - Dukung backup file besar
melalui streaming atau content-addressed storage. - Deteksi perubahan
eksternal dengan hash/mtime sebelum overwrite atau rollback. -
Definisikan kebijakan file permission, symlink, binary, dan file yang
sudah tidak ada.

**Kriteria penerimaan:** crash test pada setiap tahap write/checkpoint
menghasilkan file konsisten dan recovery dapat menentukan operasi yang
selesai atau belum selesai.

### 4.4 Perjelas shell execution boundary

Audit dan tegaskan: - Apakah command dijalankan melalui shell atau
executable plus argument. - Bagaimana working directory dibatasi. -
Apakah environment variables dan secrets diteruskan. - Apakah subprocess
dan child process dihentikan saat timeout/cancellation. - Apakah output
dibatasi. - Apakah command berbahaya memerlukan approval. - Apakah
proses dapat mengakses filesystem di luar workspace.

Gunakan command policy eksplisit, environment allowlist,
working-directory enforcement, output cap, timeout, process-group
termination, serta audit log. Prompt LLM bukan security control.

## 5. P2 --- Provider dan context management

### 5.1 Konsolidasikan LLM client

Jalur blocking dan asynchronous boleh ada jika disengaja, tetapi
kontraknya harus konsisten.

-   Tetapkan satu trait provider sebagai kontrak utama.
-   Pisahkan transport, retry policy, stream parsing, dan normalisasi
    error.
-   Standarkan cancellation dan timeout.
-   Buat contract tests untuk setiap provider adapter.
-   Isolasi dan dokumentasikan client blocking jika tetap diperlukan.

### 5.2 Tingkatkan token accounting

Estimasi `chars / 4` hanya fallback kasar, bukan angka akurat untuk
billing/quota.

Prioritas sumber usage: 1. Usage aktual dari provider. 2. Tokenizer
sesuai model jika provider tidak memberikan usage. 3. Estimasi karakter
sebagai fallback yang diberi label `estimated`.

Catat input, output, cached tokens, reasoning tokens, dan tool-related
usage jika tersedia. Jangan menyamakan estimasi dengan billing aktual.

### 5.3 Benchmark context compression dan evidence retrieval

Uji pada repository dengan pertanyaan lintas file, symbol lookup,
perubahan lintas modul, context overflow, evidence yang bertentangan,
serta direktori generated/vendor.

Metrik: - Evidence recall dan precision. - Token sebelum/sesudah
compression. - Akurasi terhadap ground truth. - Jumlah file relevan yang
ditemukan. - Latency dan biaya inference.

## 6. Rencana pengujian

### Unit tests

-   Path normalization dan workspace boundary.
-   State transition dan completion invariant.
-   Retry classification dan backoff.
-   Token usage normalization.
-   Tool argument validation.
-   Checkpoint metadata dan rollback state.

### Integration tests

-   Turn menyelesaikan task sederhana.
-   Tool call berurutan dan multi-tool.
-   Provider mengembalikan 429, 5xx, malformed stream, dan disconnect.
-   User membatalkan turn saat inference dan tool berjalan.
-   Verifier lulus, gagal, tidak tersedia, dan timeout.
-   File berubah dari luar workspace antara read dan write.

### Concurrency tests

-   Dua turn pada session sama dan berbeda.
-   Dua tool menulis file sama.
-   Cancellation bersamaan dengan tool completion.
-   Cleanup turn lama setelah turn baru dimulai.

### Fault-injection tests

-   Proses mati saat temporary file ditulis.
-   Proses mati setelah rename sebelum metadata checkpoint tersimpan.
-   Database tidak tersedia saat finalization.
-   Provider menggantung tanpa event.
-   Child process membuat child process lain.
-   Disk penuh atau permission berubah saat write.

### Security tests

-   Path traversal dan symlink escape.
-   Absolute path dan workspace root dengan prefix mirip.
-   Command injection melalui argument.
-   Environment variable leakage.
-   Output tool sangat besar.
-   MCP tool dengan side effect dan permission yang tidak sesuai.

## 7. Urutan implementasi

### Sprint 1 --- Safety dan lifecycle

-   [ ] Audit dan perbaiki `resolve_allowed_path()`.
-   [ ] Tambahkan regression tests path traversal dan symlink.
-   [ ] Terapkan per-session execution lock/actor.
-   [ ] Uji dua turn pada session sama.
-   [ ] Dokumentasikan invariant lifecycle turn.

**Definition of Done:** tidak ada operasi workspace keluar dari
boundary; satu session tidak menjalankan dua turn yang bertabrakan.

### Sprint 2 --- Cancellation dan subprocess

-   [ ] Audit seluruh penggunaan `spawn_blocking`.
-   [ ] Migrasikan streaming provider ke async-native jika memungkinkan.
-   [ ] Tambahkan cancellation token end-to-end.
-   [ ] Pisahkan connect, idle, dan total timeout.
-   [ ] Pastikan subprocess dan child process dihentikan saat
    cancellation.
-   [ ] Tambahkan test worker/child process yang menggantung.

**Definition of Done:** timeout dan cancellation menghentikan pekerjaan
secara terukur; tidak ada efek terlambat dari turn yang sudah berakhir.

### Sprint 3 --- Verification dan checkpoint

-   [ ] Buat struktur `VerificationEvidence`.
-   [ ] Tegakkan aturan `Completed` versus `CompletedUnverified`.
-   [ ] Tambahkan atomic file replacement.
-   [ ] Tambahkan durable checkpoint journal.
-   [ ] Tambahkan crash-recovery tests.
-   [ ] Definisikan kebijakan file besar dan perubahan eksternal.

**Definition of Done:** status selesai memiliki bukti yang sesuai;
perubahan file dapat dipulihkan setelah crash sesuai jaminan
terdokumentasi.

### Sprint 4 --- Performance dan observability

-   [ ] Implementasikan tool scheduler berbasis side-effect.
-   [ ] Paralelkan read-only tools dengan concurrency limit.
-   [ ] Tambahkan metrics latency per tahap agent loop.
-   [ ] Catat retry, timeout, cancellation, tool failure, dan
    verification result.
-   [ ] Tingkatkan token accounting.
-   [ ] Benchmark context retrieval dan compression.

**Definition of Done:** peningkatan latency dan biaya dapat diukur tanpa
mengurangi correctness atau safety.

## 8. Arsitektur target

``` text
User Request
    |
    v
Session Actor
    |  - one active turn per session
    |  - cancellation ownership
    |  - serialized session state
    v
Agent Runtime
    |
    +---- Context Engine
    |       - history
    |       - retrieval
    |       - compression
    |
    +---- Goal Engine
    |       - lifecycle
    |       - planning state
    |
    v
LLM Router
    |  - provider adapter
    |  - retry and timeout
    |  - usage normalization
    v
Tool Scheduler
    |
    +---- Read Pool
    +---- Mutation Queue
    +---- Process Queue
    +---- External/MCP Queue
    |
    v
Verification Engine
    |  - verifier discovery
    |  - execution
    |  - evidence persistence
    v
Completion Decision
    |
    +---- Completed
    +---- CompletedUnverified
    +---- Failed
    |
    v
Durable Checkpoint / Session Persistence
```

Prinsip desain: 1. Session Actor mengatur eksklusivitas dan lifecycle
turn. 2. Agent Runtime mengatur reasoning loop, bukan detail transport
provider. 3. LLM Router menangani provider, retry, streaming, dan usage.
4. Tool Scheduler menjadi jalur eksekusi tool dan menerapkan policy
concurrency. 5. Verification Engine menghasilkan bukti terstruktur. 6.
Checkpoint subsystem menyediakan recovery yang konsisten.

Hindari abstraksi baru hanya demi abstraksi. Setiap komponen harus
memiliki invariant, tanggung jawab, dan test yang jelas.

## 9. Yang tidak perlu diprioritaskan dulu

-   Menambah banyak tool baru.
-   Menambah banyak provider sebelum kontrak provider stabil.
-   Menambah heuristik completion tanpa evaluasi.
-   Mengaktifkan parallel tool execution secara global.
-   Menambah fitur autonomous yang memperbesar akses shell/filesystem
    sebelum sandbox aman.
-   Rewrite besar sebelum baseline tests tersedia.

Fokus pada correctness dan reliability terlebih dahulu.

## 10. Kesimpulan

Ragasukma sudah memiliki fondasi lebih serius daripada LLM wrapper
biasa: goal orchestration, verification states, evidence handling,
context management, session-aware snapshots, tool audit, dan streaming
retry.

Arah pengembangan yang disarankan adalah memperkuat enam jaminan sistem:

1.  **Safety:** agent tidak dapat keluar dari workspace atau menjalankan
    operasi tanpa policy.
2.  **Consistency:** satu session memiliki lifecycle terkontrol.
3.  **Cancellation:** pekerjaan yang dibatalkan benar-benar berhenti.
4.  **Recoverability:** perubahan file dapat dipulihkan setelah
    kegagalan.
5.  **Verifiability:** klaim selesai didukung bukti nyata.
6.  **Efficiency:** tool independen dapat berjalan paralel secara aman.

Urutan paling penting: **filesystem security → session concurrency →
stream cancellation → verification enforcement → durable checkpoint →
tool scheduling**.

Jika enam area ini ditangani dengan baik, Ragasukma akan memiliki
fondasi yang jauh lebih kuat untuk berkembang menjadi autonomous coding
agent yang dapat diandalkan.

## Catatan audit

Dokumen ini merangkum static source review terhadap bagian repository
yang diperiksa dalam percakapan. Ini bukan hasil audit menyeluruh,
penetration test, benchmark, atau bukti bahwa seluruh risiko berhasil
dieksploitasi.

Sebelum menganggap suatu temuan sebagai vulnerability terkonfirmasi: -
Cocokkan kembali dengan commit/branch yang sedang digunakan. - Tambahkan
regression test yang mereproduksi skenario. - Jalankan `cargo test` dan
pemeriksaan lint yang relevan. - Lakukan pengujian runtime pada
lingkungan terisolasi. - Catat hasil, commit, dan kondisi pengujian.

Rekomendasi ini adalah backlog engineering yang dapat diprioritaskan dan
diverifikasi, bukan klaim bahwa seluruh perbaikan wajib dilakukan dengan
desain yang persis sama.
