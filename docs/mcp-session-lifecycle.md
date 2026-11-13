# Rencana Manajemen Lifecycle & Sinkronisasi Proses MCP

Dokumen ini mencatat 5 poin arsitektur pengelolaan lifecycle proses MCP (Model Context Protocol) agar sinkron antara database SQLite (`session_mcp_servers`, `mcp_servers`) dan runtime di memori (`STDIO_PROCESS_POOL`, `SESSION_MCP_REGISTRY`) serta bebas dari *zombie/orphan process*.

---

## 1. Saat Menghapus Sesi (`delete_session` & `delete_sessions_older_than`)
Ketika sebuah sesi dihapus oleh user atau dibersihkan otomatis karena kedaluwarsa:
1. Ambil seluruh daftar `(server_id, pid)` yang terdaftar pada `session_mcp_servers WHERE session_id = ?1`.
2. Hentikan proses MCP yang sedang berjalan di runtime in-memory untuk sesi tersebut (`SESSION_MCP_REGISTRY.clear_session(session_id)` & `shutdown_session_stdio_servers(session_id)`).
3. Jika ada `pid` yang tercatat dan masih hidup di level sistem operasi, matikan proses tersebut (`kill_os_pid(pid)`).
4. Hapus seluruh record MCP pada sesi tersebut dari database (`DELETE FROM session_mcp_servers WHERE session_id = ?1`) di dalam transaksi penghapusan sesi.

---

## 2. Saat Menghapus MCP Server Global (`delete_mcp_server`)
Ketika sebuah MCP server dihapus dari katalog global:
1. Ambil seluruh pasangan `(session_id, pid)` dari `session_mcp_servers WHERE server_id = ?1`.
2. Panggil fungsi `shutdown_server_across_all_sessions(server_id)` yang bertugas:
   - Memeriksa `STDIO_PROCESS_POOL` untuk setiap key `"{session_id}::{server_id}"` maupun `"{server_id}"`, lalu menghentikan (`child.kill()`) prosesnya jika sudah telanjur *spawn*.
   - Menghapus daftar tool dan HTTP session milik server tersebut dari `SESSION_MCP_REGISTRY` di seluruh sesi.
   - Mematikan `pid` di level OS apabila masih aktif.
3. Hapus secara eksplisit dari database:
   - `DELETE FROM session_mcp_servers WHERE server_id = ?1`
   - `DELETE FROM mcp_server_tools WHERE server_id = ?1`
   - `DELETE FROM mcp_servers WHERE id = ?1`

---

## 3. Sinkronisasi Runtime MCP (Cek Status Spawn / Alive)
Menjaga konsistensi antara proses yang sudah di-*spawn* di `STDIO_PROCESS_POOL` dengan kolom `pid` di tabel `session_mcp_servers`:
1. Menyediakan mekanisme sinkronisasi runtime ↔ database untuk mengecek apakah proses di `STDIO_PROCESS_POOL` (`child.try_wait()`) atau `pid` di OS masih hidup. Jika proses sudah mati sendiri, bersihkan dari pool in-memory dan reset `pid = NULL` pada `session_mcp_servers`.
2. Setiap kali `remove_session_mcp_server` atau penonaktifan MCP pada suatu sesi dijalankan, cek langsung di runtime apakah server tersebut sudah *spawn*; jika sudah, langsung hentikan prosesnya dan set `pid = NULL`.

---

## 4. Pembersihan Orphan Process Saat Aplikasi Awal Running (Startup Sweep)
Mengatasi kondisi ketika aplikasi sebelumnya mengalami *crash* atau *force close* sehingga proses MCP child masih hidup di background:
1. Saat aplikasi pertama kali dijalankan (startup), jalankan fungsi `cleanup_orphaned_mcp_processes_on_startup()`.
2. Query seluruh baris `SELECT session_id, server_id, pid FROM session_mcp_servers WHERE pid IS NOT NULL`.
3. Untuk setiap `pid` yang ditemukan, lakukan terminasi proses di level OS (Unix: `kill -9 <pid>`, Windows: `taskkill /F /T /PID <pid>`).
4. Reset seluruh kolom PID di database (`UPDATE session_mcp_servers SET pid = NULL`) agar state awal aplikasi bersih.

---

## 5. Mematikan Semua MCP Saat Aplikasi Ditutup (Graceful Shutdown)
Memastikan tidak ada proses MCP yang tertinggal ketika user menutup aplikasi (`ragasukma` desktop/web maupun `ragasukma-tui`):
1. Sediakan fungsi terpadu `shutdown_all_mcp_servers()` yang:
   - Mematikan seluruh child process aktif di `STDIO_PROCESS_POOL` (`shutdown_all_stdio_servers()`).
   - Meng-kill seluruh `pid` yang masih tercatat di `session_mcp_servers WHERE pid IS NOT NULL` dan meng-update `pid = NULL` di database.
2. Pasang pemanggilan fungsi ini pada lifecycle exit/shutdown aplikasi dan signal handler (`Ctrl+C` / `SIGTERM`).
