# Temuan Bug: Siklus Hidup Turn & Workspace Snapshot

> **Status:** Selesai diperbaiki & terverifikasi — Seluruh Tahap 1–4 (BUG-01 s/d BUG-11) **selesai & terverifikasi penuh** (191 tests passed).
> **Cakupan:** `ragasukma-core` (`src/agent/executor`, `src/agent/snapshot.rs`, `src/agent/tools`, `src/agent/maintenance.rs`, `src/agent/repository`) dan `ragasukma-tui` (`src/core_bridge/worker.rs`)
> **Sifat:** Audit terarah pada area siklus hidup turn (start → error → selesai) dan mekanisme rollback file.
> **Tanggal temuan:** sesi audit dokumentasi awal.
> **Terakhir diperbarui:** setelah implementasi menyeluruh Tahap 1–4 (lihat §5 untuk bukti verifikasi komprehensif).


---

## 0. Ringkasan

Akar masalah dari seluruh temuan di bawah ini hanya **dua**:

1. **Semantik kepemilikan snapshot salah** — store backup file dibuat **global** (satu untuk seluruh proses), padahal fungsinya per-sesi. Ini memunculkan BUG-02 dan BUG-05.
2. **Penanganan error tidak konsisten** — ada `let _ =` (error dibuang) di jalur yang justru kritis, dan ada `?` (early return) di jalur yang seharusnya ditangani. Ini memunculkan BUG-01, BUG-03, BUG-04, BUG-06, BUG-07, BUG-10.

### Tabel Prioritas & Status

| ID | Bug | Lokasi | Dampak | Prioritas | Status |
|---|---|---|---|---|---|
| BUG-03 | Sesi stuck `"running"` permanen saat DB/tool error | `executor/mod.rs`, `maintenance.rs` | UI macet, butuh restart | 🔴 Kritis | ✅ Selesai (Tahap 1) |
| BUG-04 | Cancel turn tidak mengirim event apa pun | `worker.rs:433-443`, `:85-88` | Spinner tidak berhenti | 🔴 Kritis | ✅ Selesai (Tahap 1) |
| BUG-02 | Backup sesi paralel saling menghapus | `snapshot.rs`, `tools/mod.rs` | Rollback mustahil antar sesi | 🔴 Kritis | ✅ Selesai (Tahap 3) |
| BUG-01 | `rollback_all` melaporkan "success" palsu | `snapshot.rs`, `tools/mod.rs` | Kehilangan data senyap | 🔴 Kritis | ✅ Selesai (Tahap 2) |
| BUG-05 | Kontrak tool `workspace_rollback` tidak sesuai realita | `tools/mod.rs`, `executor/mod.rs` | Fitur undo tidak pernah berfungsi lintas turn | 🟠 Tinggi | ✅ Selesai (Tahap 3) |
| BUG-06 | Kegagalan backup diabaikan, write tetap jalan | `tools/mod.rs` | Perubahan permanen tanpa undo | 🟠 Tinggi | ✅ Selesai (Tahap 4) |
| BUG-07 | `?` pada release menutupi error asli | `executor/mod.rs` | Pesan error menyesatkan | 🟠 Tinggi | ✅ Selesai (Tahap 2) |
| BUG-08 | Tidak ada batas ukuran backup | `snapshot.rs` | Konsumsi RAM tak terbatas | 🟡 Sedang | ✅ Selesai (Tahap 4) |
| BUG-09 | Test mengunci perilaku buggy | `executor/mod.rs` | Menghalangi perbaikan BUG-05 | 🟡 Sedang | ✅ Selesai (Tahap 3) |
| BUG-10 | Kegagalan hapus file saat rollback diabaikan | `snapshot.rs` | Klaim "berhasil" palsu | 🟡 Sedang | ✅ Selesai (Tahap 2) |
| BUG-11 | `git init` berjalan tiap turn, hasilnya dibuang | `executor/mod.rs:296-306` | Efek samping tak terlihat | 🔵 Catatan | ✅ Selesai (Tahap 4) |

---

## 1. Peta Alur yang Relevan

```
TUI: user tekan Enter
  └─ CoreBridgeCommand::ExecuteTurn            (worker.rs:703)
       ├─ set_active_status(..., "running")    (worker.rs:717)  → UI spinner ON
       ├─ create_message("user", prompt)       (worker.rs:718)
       └─ tokio::spawn(execute_turn_pipeline)  (worker.rs:737)
                                  │
                                  ▼
CORE: execute_session_turn()                   (executor/mod.rs:36)
  ├─ :62   update_session_status("running")    → DB status = running
  ├─ :75   list_messages_paginated().await?    ← early-exit #1  (BUG-03)
  ├─ :83   resolve_or_create_goal().await?     ← early-exit #2  (BUG-03)
  ├─ :101  build_system_prompt().await?        ← early-exit #3  (BUG-03)
  ├─ LOOP
  │    ├─ :321 create_message(assistant).await?   ← early-exit #4  (BUG-03)
  │    ├─ :338 execute_turn_tool_calls().await?   ← early-exit #5  (BUG-03)
  │    └─ tools/mod.rs → snapshot.backup_file() → fs::write
  ├─ :240  release_turn_snapshots()?  ← jalur error LLM → clear() store GLOBAL
  ├─ :439  create_message(final).await?           ← early-exit #6  (BUG-03)
  ├─ :449  update_session_status("idle")
  ├─ :457  release_turn_snapshots()?  ← jalur sukses → clear() store GLOBAL
  └─ :462  release_turn_snapshots()?  ← jalur fallback batas iterasi
```

---

## 2. Detail Temuan

### 🔴 BUG-01 — `rollback_all` melaporkan "success" padahal restore gagal

**Lokasi:** `src/agent/snapshot.rs:83-99`

```rust
for (path, original) in backups.drain() {
    match original {
        Some(bytes) => {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);   // :87  error dibuang
            }
            let _ = std::fs::write(&path, bytes);          // :89  error dibuang
        }
        None => {
            if path.exists() {
                let _ = std::fs::remove_file(&path);       // :93  error dibuang
            }
        }
    }
    restored.push(path);                                   // :97  tetap dihitung berhasil
}
```

**Bukti ketidakkonsistenan:** fungsi kembarnya, `rollback_file` (`:56-70`), **melakukan hal yang benar** dan memakai `?`:

```rust
std::fs::create_dir_all(parent)?;   // :60
std::fs::write(&abs_path, bytes)?;  // :62
```

Jadi dalam satu file yang sama: `rollback_file` jujur, `rollback_all` menipu.

**Dampak:** `tools/mod.rs:590-601` menerjemahkan hasilnya menjadi JSON ke LLM:

```json
{ "status": "success", "restored_count": 3, "restored_files": ["...", "...", "..."] }
```

Padahal file bisa gagal ditulis (read-only, disk penuh, file terkunci antivirus/editor di Windows). Agent lalu melaporkan ke user bahwa perubahan "sudah dikembalikan" — **laporan palsu yang tidak terdeteksi**.

**Reproduksi:**
1. Buat file `a.txt`, jalankan agent sehingga file tercatat di snapshot.
2. Set file menjadi read-only (`attrib +R a.txt`).
3. Panggil `workspace_rollback` tanpa `file_path`.
4. Amati: response `status: "success"`, `restored_count: 1`, tetapi isi file tidak berubah.

**Arah perbaikan:** kumpulkan kegagalan per-path, kembalikan `Vec<(PathBuf, io::Error)>` atau struct hasil yang membedakan `restored` vs `failed`, lalu laporkan `status: "partial"` bila ada yang gagal. Jangan `push` ke `restored` kecuali operasi disk sukses.

---

### 🔴 BUG-02 — Snapshot global: backup sesi paralel saling menghapus

**Lokasi:** `src/agent/snapshot.rs:6` dan `:26`

```rust
static GLOBAL_SNAPSHOT: OnceLock<WorkspaceSnapshot> = OnceLock::new();   // :6

pub fn global() -> &'static WorkspaceSnapshot {                          // :26
    GLOBAL_SNAPSHOT.get_or_init(Self::new)
}
```

`static` + `OnceLock` berarti: **satu instance untuk seluruh proses**, hidup sampai aplikasi ditutup. Tidak ada dimensi `session_id` maupun `project_id`.

**Sementara aplikasi memang menjalankan banyak sesi sekaligus:**
- Turn di-`tokio::spawn` per sesi (`worker.rs:737`)
- `RUNNING_SESSION_TASKS` / `TaskGroup` di-key per `session_id` (`worker.rs:82`)

**Rantai kegagalan:**

```
Sesi A: edit config.rs  →  store global: { config.rs → byte asli }
Sesi B: edit main.rs    →  store global: { config.rs → ..., main.rs → byte asli }

Sesi B selesai lebih dulu
  └─ executor/mod.rs:457  release_turn_snapshots()
       └─ snapshot.rs:118  clear()  →  SELURUH map dikosongkan
            └─ backup config.rs milik Sesi A  IKUT HILANG

Sesi A perlu rollback  →  Ok(false)  (snapshot.rs:72)
```

Analoginya: satu papan tulis untuk seluruh gedung. Ruangan B selesai rapat, penghapusnya menghapus seluruh papan — termasuk catatan ruangan A yang rapatnya belum selesai.

**Arah perbaikan:** ubah menjadi `HashMap<SessionId, WorkspaceSnapshot>` dengan `OnceLock<Mutex<...>>`, atau teruskan `session_id` ke `execute_turn_tool_calls` sehingga snapshot bisa di-scope. Catatan: saat ini `resolve_allowed_path`/tool executor **tidak** menerima `session_id` — perlu penelusuran parameter dari `executor/mod.rs:329` ke bawah.

---

### 🔴 BUG-03 — Error DB/tool membuat sesi stuck `"running"` permanen

**Lokasi:** `src/agent/executor/mod.rs`

Status `"running"` ditulis ke DB di `:62`:

```rust
let _ = repository::update_session_status(session_id, "running").await;
```

Setelah itu ada **enam** titik `?` yang keluar dari fungsi **tanpa pernah** mengembalikan status ke `"idle"`:

| Baris | Operasi |
|---|---|
| `:81` | `list_messages_paginated(...).await?` |
| `:83` | `state::resolve_or_create_goal(...).await?` |
| `:101` | `prompt::build_system_prompt(...).await?` |
| `:321` | `repository::create_message(assistant_msg...).await?` |
| `:338` | `tool_runner::execute_turn_tool_calls(...).await?` |
| `:439` | `repository::create_message(final_msg...).await?` |

Reset ke `"idle"` **hanya** ada di tiga tempat, dan tidak satu pun menjangkau jalur di atas:

- `:226` → hanya untuk **error LLM** (blok `match turn_res { Err(e) => ... }` di `:203-242`)
- `:425` → jalur khusus
- `:449` → jalur sukses

Blok penanganan error di `:203-241` **hanya membungkus panggilan ke model**, bukan panggilan repository maupun tool runner.

**Rantai akibat:**

```
create_message / execute_turn_tool_calls gagal (DB locked, disk penuh, dsb.)
  └─ ?  → return Err  (lewati semua reset idle DAN release snapshot)
       ├─ DB session.status tetap "running"        → permanen
       └─ snapshot tidak pernah di-clear            → memory leak per turn gagal

TUI muat ulang daftar sesi:
  └─ bridge.rs:329   let is_running = s.status == "running";   → true
       ├─ sidebar.rs:228  menampilkan tag RUNNING
       └─ spinner berputar terus
```

Karena `is_running` di sisi TUI dipulihkan **dari kolom `status` di DB** (`bridge.rs:329`), status basi itu ikut terbaca lagi **setiap kali aplikasi dijalankan ulang**. Tidak ada startup sweep yang mereset `status = 'running'` menjadi `idle`. Artinya sesi bisa tampak "running selamanya" bahkan setelah restart.

**Reproduksi:**
1. Jalankan satu turn sampai masuk fase tool execution.
2. Kunci file database SQLite dari proses lain, atau buat `RAGASUKMA_*` path tidak writable.
3. Amati: `TurnFailed` diterima TUI, tetapi setelah `LoadHistory`, status sesi kembali `running`.

**Arah perbaikan:** bungkus seluruh isi `execute_session_turn` setelah `:62` dalam satu scope yang menjamin reset. Pola paling sederhana: pindahkan badan fungsi ke `execute_session_turn_inner(...)` dan buat wrapper yang selalu melakukan `update_session_status(..., "idle")` + `release_turn_snapshots()` pada **semua** exit path (termasuk `Err`). Alternatif idiomatis Rust: guard struct dengan `Drop`.

---

### 🔴 BUG-04 — Cancel turn yang tidak aktif tidak mengirim event apa pun

**Lokasi:** `ragasukma-tui/src/core_bridge/worker.rs:433-443`

```rust
CoreBridgeCommand::CancelTurn { session_id } => {
    let sid = match session_id {
        Some(sid) => sid,                                   // ← tidak set idle, tidak emit event
        None => {
            set_active_status(&worker_tx, &active_sid, &active_status, "idle");
            active_sid.clone()
        }
    };
    active_turns.cancel(&sid);                              // abort() → tidak ada event
}
```

Dua masalah:

1. Cabang `Some(sid)` **tidak** menyentuh `set_active_status` dan **tidak** mengirim `StatusChange`.
2. `TaskGroup::cancel` (`worker.rs:82`) hanya memanggil `handle.abort()`. Task yang di-abort **tidak punya kesempatan** mengirim event.

**Konsekuensi di TUI:** satu-satunya cara `is_running` menjadi `false` adalah lewat event:

| Event | Lokasi |
|---|---|
| `TurnCompleted` | `bridge.rs:111` |
| `TurnCancelled` | `bridge.rs:157` |
| `TurnFailed` | `bridge.rs:172` |

`CoreBridgeEvent::TurnCancelled` sudah didefinisikan (`types.rs:142`) dan sudah ditangani (`bridge.rs:154`), **tetapi tidak ada satu pun kode yang mengirimnya**. Grep menunjukkan hanya dua kemunculan: definisi dan handler — nol pengirim.

Akibatnya: user menekan Cancel, task di-abort, **tidak ada event** → spinner berputar terus, dan status DB tetap `"running"` (memperparah BUG-03).

Hal yang sama berlaku untuk `TaskGroup::insert` (`worker.rs:85-88`) yang meng-abort turn lama tanpa event pembatalan.

**Arah perbaikan:** setelah `active_turns.cancel(&sid)`, selalu kirim `CoreBridgeEvent::TurnCancelled { session_id: sid }` dan set status DB ke `idle`. Untuk kasus abort, pertimbangkan menambahkan handler yang menangkap pembatalan (misalnya `tokio::select!` dengan `CancellationToken`) agar status bisa dibersihkan dari dalam task.

---

### 🟠 BUG-05 — Kontrak tool `workspace_rollback` tidak sesuai realita

**Lokasi:** `src/agent/tools/mod.rs:161,167` vs `src/agent/executor/mod.rs:240,457,462`

Deskripsi yang dibaca LLM (`tools/mod.rs:161`):

> *"Revert files to the state they were in before **this session** modified them. Every write and edit is backed up automatically. Pass 'file_path' to revert a single file, or omit it to revert every file **this session** touched."*

Deskripsi parameter (`:167`):

> *"Omit to revert every file modified during **this session**."*

Realita: backup dihapus di **setiap akhir turn**, bukan akhir sesi (`executor/mod.rs:240, 457, 462` → `clear()`).

**Rantai kegagalan:**

```
Turn 1:
  agent edit config.rs   →  backup tersimpan di store global
  turn selesai           →  :457 release_turn_snapshots() → clear()  →  BACKUP HILANG

Turn 2:
  user: "batalkan perubahan yang tadi"
  agent panggil workspace_rollback
    └─ snapshot.rs:56  backups.remove(&abs_path)  →  None
    └─ snapshot.rs:72  Ok(false)
    └─ tools/mod.rs:583  { "status": "success", "restored": false }
```

**Tool yang menjanjikan "undo" justru kehilangan datanya tepat sebelum dipakai.** Ini masalah semantik, bukan sekadar kualitas kode: pilih salah satu —

- **(A)** Perbaiki lifecycle: pertahankan backup sampai sesi benar-benar berakhir.
- **(B)** Ubah kontrak: deskripsi tool menyebut *"this turn"*, dan reset memang per turn.

Saat ini kode memilih (B) secara diam-diam sementara dokumentasi tool menjanjikan (A).

---

### 🟠 BUG-06 — Kegagalan backup diabaikan, tetapi file tetap ditulis

**Lokasi:** `src/agent/tools/mod.rs:562` (`workspace_write_file`) dan `:620` (`workspace_edit_file`)

```rust
let resolved = resolve_allowed_path(file_path, allowed_roots)?;
let _ = crate::agent::snapshot::WorkspaceSnapshot::global().backup_file(&resolved);
std_fs::write(&resolved, content)?;          // ← tetap menulis walau backup gagal
```

`backup_file` (`snapshot.rs:32-47`) mengembalikan `Result`, tetapi hasilnya dibuang dengan `let _ =`. Bila backup gagal (permission, I/O, mutex poison), **file tetap ditimpa tanpa cadangan apa pun** — perubahan menjadi permanen dan tidak bisa di-rollback.

Ini bertentangan dengan deskripsi tool yang sama yang menyatakan *"Every write and edit is backed up automatically"*.

**Arah perbaikan:** propagasi error, atau minimal catat ke log + kembalikan penanda `"backed_up": false` di response JSON agar LLM tahu rollback tidak akan tersedia untuk file tersebut.

---

### 🟠 BUG-07 — `?` pada release menutupi error asli

**Lokasi:** `src/agent/executor/mod.rs:240-241` (jalur error LLM)

```rust
release_turn_snapshots()?;
return Err(anyhow::anyhow!(err_str));
```

Bila `release_turn_snapshots()` gagal (mis. mutex poisoned — dan `snapshot.rs:118-125` memang mengembalikan `Err` untuk kasus itu), fungsi keluar dengan `Err("Mutex poisoned: ...")`. Error model yang sebenarnya (`err_str`, sudah diformat ke pesan user di `:205-208`) **tidak pernah terkirim** ke TUI.

User melihat pesan mutex, bukan penyebab sebenarnya. Pola sama di `:457` dan `:462`.

**Arah perbaikan:** `let _ = release_turn_snapshots();` pada jalur yang sudah membawa error, atau gunakan `.ok()` dan log terpisah.

---

### 🟡 BUG-08 — Tidak ada batas ukuran backup

**Lokasi:** `src/agent/snapshot.rs:40`

```rust
let content = if e.key().exists() {
    Some(std::fs::read(e.key())?)   // :40  baca SELURUH file ke RAM
} else {
    None
};
```

Tanpa batas ukuran. Bila agent menyentuh file besar (log 500 MB, binary, dataset), seluruh isinya disimpan di memori sampai turn berakhir. Dengan banyak file, konsumsi RAM bisa melonjak dan berujung OOM.

Catatan: karena BUG-03 dan BUG-07, ada jalur di mana `clear()` tidak pernah dijalankan sama sekali — sehingga pemakaian memori tidak kembali.

**Arah perbaikan:** batas per file (mis. 2 MB), dan untuk file yang lebih besar simpan di temp dir atau tandai "tidak bisa di-snapshot" agar dilaporkan ke LLM.

---

### 🟡 BUG-09 — Test mengunci perilaku yang buggy

**Lokasi:** `src/agent/executor/mod.rs:475-501`

```rust
#[test]
fn a_finished_turn_leaves_no_backups_behind() {
    ...
    WorkspaceSnapshot::global().backup_file(&file).unwrap();
    release_turn_snapshots().expect("turn snapshots must be released");
    assert!(
        WorkspaceSnapshot::global().tracked_files().unwrap().is_empty(),
        "the global store must be released when the turn ends"     // :498
    );
}
```

Test ini **meng-assert bahwa turn boundary mengosongkan store** — yaitu persis perilaku yang membuat BUG-05 terjadi. Artinya selama test ini dipertahankan, memperbaiki lifecycle rollback akan **selalu** memerahkan test suite.

Dua catatan tambahan:
- Test memakai `WorkspaceSnapshot::global()` yang **dibagi antar test** dalam satu proses → rawan flaky bila `cargo test` berjalan paralel dengan test snapshot lain (`snapshot.rs:142,148,188-205`).
- Komentar di `snapshot.rs:11-13` dan `executor/mod.rs:22-27` menyatakan bahwa perilaku ini sengaja dipilih **untuk mencegah pertumbuhan memori** — jadi keputusan ini sadar, tetapi bertabrakan dengan kontrak tool.

**Arah perbaikan:** bila BUG-05 diselesaikan dengan opsi (A), test ini harus ditulis ulang menjadi "a finished **session** leaves no backups behind". Bila memilih opsi (B), cukup perbaiki deskripsi tool — test boleh tetap.

---

### 🟡 BUG-10 — Kegagalan hapus file saat rollback diabaikan

**Lokasi:** `src/agent/snapshot.rs:66` dan `:93`

```rust
None => {
    if abs_path.exists() {
        let _ = std::fs::remove_file(&abs_path);   // :66
    }
}
// dan
Ok(true)                                            // :70  selalu "berhasil"
```

Entri `None` berarti file **tidak ada sebelum agent menulisnya**. Rollback seharusnya menghapus file itu. Bila `remove_file` gagal (file terkunci proses lain — sangat umum di Windows), fungsi tetap mengembalikan `Ok(true)` dan `tools/mod.rs:585` melaporkan `"restored": true`.

Sama seperti BUG-01, ini klaim berhasil yang tidak benar.

---

### 🔵 BUG-11 — `git init` berjalan setiap turn dan hasilnya dibuang

**Lokasi:** `src/agent/executor/mod.rs:50-52`

```rust
for folder in &allowed_folders {
    let _ = crate::agent::tools::ensure_git_and_gitignore(std::path::Path::new(folder));
}
```

Setiap turn, untuk **setiap** folder root, fungsi ini dijalankan. `ensure_git_and_gitignore` (`tools/fs.rs:293`) meneruskan `auto_git_init = true` secara hardcode ke `ensure_git_and_gitignore_opt`, yang menjalankan subprocess `git init` bila `.git` tidak ada.

Hasil (`Ok`/`Err`) dibuang total dengan `let _ =`.

**Catatan:** kemungkinan ini memang disengaja (auto-inisialisasi workspace). Namun perlu dikonfirmasi karena:
- Menjalankan subprocess `git init` di setiap turn menambah latensi yang tidak perlu.
- Kegagalan (mis. `git` tidak terpasang) tidak pernah dilaporkan ke user.
- Ini **efek samping tak terlihat** ke folder user — menulis direktori `.git` — tanpa jejak di log.

---

## 3. Rencana Perbaikan Bertahap

Urutan diusulkan berdasarkan rasio dampak/risiko, bukan semata prioritas.

### Tahap 1 — Pulihkan konsistensi status (BUG-03, BUG-04)
Tidak menyentuh semantik snapshot, jadi risiko rendah dan langsung terasa oleh user.

1. `executor/mod.rs` — refaktor `execute_session_turn` menjadi wrapper + `_inner`, sehingga `update_session_status("idle")` dan `release_turn_snapshots()` **selalu** berjalan.
2. `worker.rs:442` — kirim `CoreBridgeEvent::TurnCancelled` setelah cancel, dan set status DB `idle`.
3. `worker.rs:85-88` — perlakuan sama untuk abort pada `TaskGroup::insert`.
4. Pertimbangkan startup sweep: `UPDATE sessions SET status='idle' WHERE status='running'` saat init, karena status basi bertahan lintas restart.

### Tahap 2 — Jujur soal kegagalan (BUG-01, BUG-07, BUG-10)
1. `snapshot.rs` — `rollback_all` kumpulkan kegagalan, jangan `push` ke `restored` bila gagal.
2. `snapshot.rs:66` — propagasi error `remove_file`.
3. `executor/mod.rs:240,457,462` — jangan pakai `?` pada release di jalur yang sudah membawa error.

### Tahap 3 — Keputusan desain snapshot (BUG-02, BUG-05, BUG-09)
Butuh keputusan produk lebih dulu: **rollback itu per-turn atau per-sesi?**

- Opsi **A (per-sesi)**: `HashMap<SessionId, WorkspaceSnapshot>`; `session_id` harus diteruskan ke `execute_turn_tool_calls`; BUG-09 test ditulis ulang.
- Opsi **B (per-turn)**: perbaiki deskripsi tool `tools/mod.rs:161,167` menjadi "this turn"; test BUG-09 dipertahankan.

### Tahap 4 — Pengerasan (BUG-06, BUG-08, BUG-11)
1. `tools/mod.rs:562,620` — jangan buang error backup.
2. `snapshot.rs:40` — batas ukuran file.
3. `executor/mod.rs:51` — log hasil `ensure_git_and_gitignore`, atau pindahkan keluar dari hot path turn.

---

## 4. Catatan Metodologi

- Seluruh nomor baris diverifikasi langsung terhadap isi file pada saat audit dan implementasi dilakukan.
- Klaim "tidak ada pengirim `TurnCancelled`" diverifikasi lewat grep menyeluruh `ragasukma-tui/src` — hasil awal: hanya `types.rs:142` (definisi) dan `bridge.rs:154` (handler), kini sudah diperbaiki dengan emisi aktif di `worker.rs`.
- Radius audit terfokus pada `snapshot.rs`, `executor/`, `tools/mod.rs`, `maintenance.rs`, dan `core_bridge/worker.rs`.
- Semua perbaikan divalidasi dengan unit test lokal dan regression test suite komprehensif.

---

## 5. Status Implementasi & Verifikasi Menyeluruh (Tahap 1–4 Selesai)

Seluruh temuan bug pada Tahap 1, 2, 3, dan 4 telah diimplementasikan secara tuntas di `ragasukma-core` dan `ragasukma-tui`.

### Tahap 1 — Konsistensi Status Turn & Event Pembatalan · **SELESAI**

- **BUG-03 (Sesi stuck `"running"` permanen)**:
  - **`TurnStatusGuard`** (`src/agent/executor/mod.rs`): Menjamin pemulihan status sesi ke `idle` serta broadcast event `StatusChange` pada setiap jalur keluar (baik `Ok`, `Err`, maupun abort/panic via `Drop`).
  - **Generation Counter** (`claim_turn_generation`, `is_latest_turn`): Mencegah race condition ketika cleanup turn lama berjalan lambat agar tidak menimpa status `running` milik turn baru yang sedang aktif.
  - **Startup Sweep** (`reset_stale_running_sessions` di `src/agent/maintenance.rs`): Mereset sesi yang tertinggal dalam status `running` akibat aplikasi crash/force-kill sebelumnya saat startup.
- **BUG-04 (Turn cancelation tanpa event)**:
  - `ragasukma-tui/src/core_bridge/worker.rs`: Memancarkan event `CoreBridgeEvent::TurnCancelled { session_id }` dan memutakhirkan status sesi ke `idle` ketika user membatalkan turn.
  - Penanganan nilai balik `active_turns.cancel()` memastikan event pembatalan tidak memicu false cancel pada turn yang telah tuntas.

### Tahap 2 — Kejujuran Pelaporan Hasil Rollback · **SELESAI**

- **BUG-01 & BUG-10 (Laporan palsu `rollback_all` & kegagalan `remove_file`)**:
  - `src/agent/snapshot.rs`: Diperkenalkan struct `RollbackReport` dengan kategori eksplisit: `restored: Vec<PathBuf>`, `failed: Vec<(PathBuf, String)>`, dan `not_found: Vec<PathBuf>`.
  - Operasi disk (`create_dir_all`, `write`, `remove_file`) tidak lagi membuang error dengan `let _ =`. Jika penghapusan file baru yang ditambahkan agent gagal (misal file terkunci), error dicatat ke daftar `failed`.
  - `src/agent/tools/mod.rs`: `workspace_rollback` melaporkan status `"partial"` apabila terdapat kegagalan restore, menyertakan daftar path dan pesan error, serta secara eksplisit menginstruksikan LLM untuk mengabarkan kegagalan ini kepada pengguna (bukan mengklaim sukses).
- **BUG-07 (`?` pada release snapshot menutupi error asli)**:
  - `src/agent/executor/mod.rs`: Pembersihan snapshot tidak lagi ditempatkan dengan `?` di jalur propagasi error LLM. Error asli dari eksekusi model/tools selalu diteruskan utuh tanpa terselubung oleh error manajemen snapshot.

### Tahap 3 — Isolasi Snapshot & Penyelarasan Kontrak Undo · **SELESAI**

- **BUG-02 & BUG-05 (Isolasi snapshot paralel & kontrak per-sesi)**:
  - Memilih **Opsi A (Rollback Per-Sesi)**: Sesuai kontrak tool bahwa rollback membatalkan perubahan *"during this session"*.
  - `src/agent/snapshot.rs`: `GLOBAL_SNAPSHOT` tunggal diganti menjadi store aman terisolasi per sesi: `static SESSIONS: OnceLock<Mutex<HashMap<String, WorkspaceSnapshot>>>`.
  - Disediakan `WorkspaceSnapshot::for_session(session_id)` dan `release_session(session_id)`.
  - Pemanggilan tool executor (`execute_tool_call` & `execute_turn_tool_calls`) kini membawa parameter `session_id`, sehingga sesi paralel tidak saling menimpa atau menghapus backup satu sama lain.
  - Snapshot backup dipertahankan melintasi batas turn dan hanya dibersihkan secara aman ketika sesi dihapus (`repository::delete_session`).
- **BUG-09 (Pembaruan test boundary snapshot)**:
  - Test usang yang memaksakan store kosong di setiap turn diubah menjadi pengujian isolasi sesi (`release_session_snapshots`) dan ketahanan backup lintas turn.

### Tahap 4 — Pengerasan Keandalan & Pencegahan OOM · **SELESAI**

- **BUG-06 (Validasi backup sebelum penulisan)**:
  - `src/agent/tools/mod.rs`: Di dalam tool `workspace_write_file` dan `workspace_edit_file`, error dari `backup_file` tidak lagi diabaikan. Jika backup gagal (misal file terkunci atau disk error), operasi write/edit segera dibatalkan dengan pesan error kontekstual sebelum file asli tersentuh.
- **BUG-08 (Batas ukuran backup RAM)**:
  - `src/agent/snapshot.rs`: Ditetapkan konstanta `MAX_BACKUP_BYTES = 2 * 1024 * 1024` (2 MiB).
  - Tipe enum `BackupState` mendukung varian `Captured(Vec<u8>)`, `MissingBeforeModification`, dan `Oversized { size: u64 }`.
  - File yang melebihi batas ukuran tidak dimuat seluruhnya ke RAM (mencegah memory spike / OOM), tetapi tetap dicatat sebagai `Oversized`. Saat rollback dicoba, sistem dengan jujur melaporkan file tersebut gagal direstore karena melebihi batas snapshot.
- **BUG-11 (Transparansi inisialisasi workspace git)**:
  - `src/agent/executor/mod.rs`: Eksekusi `ensure_git_and_gitignore` kini menangkap error dan mencatatnya ke `tracing::warn!`, sehingga masalah izin atau kegagalan inisialisasi workspace git terpantau dan tidak hilang senyap.

---

### Bukti Verifikasi Pengujian

| Komponen / Test Suite | Cakupan Uji | Hasil |
|---|---|---|
| `cargo test` (`ragasukma-core`) | 191 unit tests (termasuk snapshot, executor guard, tools, goal tx, dsb.) | **PASSED** (191 passed; 0 failed; 0 ignored) |
| Unit Test Baru `snapshot::tests` | Uji Poisoned Mutex recovery, Partial failure reporting, Oversized file safety boundary | **PASSED** |
| Unit Test Baru `executor::tests` | Uji race-condition pencegahan stale generation cleanup (N-2) & session snapshot lifecycle | **PASSED** |

### Status Deliverable Saat Ini

- Seluruh kode perbaikan Tahap 1 s/d 4 telah selesai diimplementasikan.
- Working copy saat ini berstatus unstaged changes (tidak dicommit sesuai permintaan pengguna).