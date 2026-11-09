# Refactor #2 — Move the database pool out of `llm` and break the `repository` ⇄ `llm` cycle

**Status: PLAN (not yet executed)**

## 1. Problem

The pool that every repository function uses does not live with the repository layer. It
lives in `src/llm/usage.rs`, next to LLM usage accounting:

```rust
// src/llm/usage.rs
static POOL: OnceCell<SqlitePool> = OnceCell::const_new();   // line 46

#[cfg(test)]                                                 // line 53
pub async fn get_pool() -> &'static SqlitePool { ... }

#[cfg(not(test))]                                            // line 249
pub async fn get_pool() -> &'static SqlitePool { ... }

pub fn get_db_path() -> PathBuf { ... }                      // line 213
```

This forces a dependency cycle between two layers that should not know about each other:

| Direction | Call sites | Meaning |
|---|---|---|
| `repository` → `llm::usage::get_pool()` | **5** (`app/chat_history.rs` ×4, `agent/mod.rs` ×1) | data layer depends on the LLM layer |
| `llm` → `repository::chat_history` | **1** (`llm/mod.rs` `history::log_conversation_turn`) | LLM layer depends on the data layer |

Neither dependency is about LLM behaviour or data storage. One is "where is the pool",
the other is a logging convenience wrapper. The practical cost is that `repository` cannot
be built, read, or tested without dragging in the whole `llm` module.

A related smell: `usage.rs` is 730 lines and carries two concerns. Lines 45–291 are pure
database plumbing (`POOL`, both `get_pool` variants, `test_pool`, `TempDbGuard`,
`reap_stale_test_dbs`, `process_is_alive`, `should_reap`, `get_db_path`); the rest is
actual usage accounting. The DB half has nothing to do with usage accounting.

## 2. Target layout

Create one new module that owns the pool, the database path, and the test database:

```
src/
  db.rs                    ← NEW: pool + path + test db lifecycle (moved from llm/usage.rs)
  llm/
    usage.rs               ← usage accounting only; imports `crate::db::get_pool()`
  repository/
    agent/mod.rs           ← `pool()` delegates to `crate::db::get_pool()`
    app/chat_history.rs    ← 4 call sites updated
  lib.rs                   ← `pub mod db;`
```

Dependency after the change:

```
agent ──► llm ──► db          (llm keeps using the pool for usage rows)
  └────► repository ──► db
repository ──► llm            ← REMOVED
```

`llm/mod.rs::history` still re-exports into `repository::chat_history`. That is the
remaining edge and is handled separately in §4; it does **not** re-form a cycle once
`repository` no longer points back at `llm`.

## 3. What moves, exactly

Move verbatim from `src/llm/usage.rs` into `src/db.rs`:

| Item | Current lines | Notes |
|---|---|---|
| `static POOL` | 45–46 | drop `#[allow(dead_code)]` if it becomes unnecessary |
| `#[cfg(test)] get_pool` | 48–56 | keep the doc comment about never touching the live DB |
| `#[cfg(test)] test_pool` | 58–102 | |
| `reap_stale_test_dbs`, `should_reap`, `process_is_alive` | 103–196 | test-only helpers |
| `TempDbGuard` + `Drop` + `TEST_DB_GUARD` | 197–211 | test-only |
| `get_db_path` | 212–247 | public, used only by `get_pool` |
| `#[cfg(not(test))] get_pool` | 249–291 | |

Everything from `record_usage_async` (line 292) onwards **stays** in `usage.rs`.

### Public surface to preserve

`get_db_path()` is currently reachable at the path `crate::llm::usage::get_db_path`. Check
whether anything outside the crate depends on that path (a `bridge.rs` or TUI in another
crate). If it is used externally, either:
- re-export from `usage.rs` with `pub use crate::db::get_db_path;`, or
- keep both paths compiling via `#[deprecated]` alias for one release.

Inside this crate the only consumers of `get_pool` are the 5 repository call sites listed
in §1, so the internal migration is small.

## 4. Second edge: `llm/mod.rs::history`

```rust
// src/llm/mod.rs
pub mod history {
    pub fn log_conversation_turn(...) {
        crate::repository::chat_history::log_conversation_turn(...)
    }
}
```

Once `repository` stops depending on `llm`, this edge is a plain one-way call and no longer
forms a cycle. Two options, in order of preference:

1. **Move the wrapper to the caller.** `history::log_conversation_turn` is a one-line
   forwarder. If callers (bridge/TUI) can call `repository::chat_history::log_conversation_turn`
   directly, delete the wrapper and the last `llm → repository` edge disappears too.
2. **Keep it** as a compatibility shim, documented as "convenience, not a dependency".

Decide before starting so §5 does not stall.

## 5. Execution stages

**Stage A — introduce `src/db.rs` (pure move, no logic change).**
Create `db.rs` with the items in §3, add `pub mod db;` to `lib.rs`, make `usage.rs` import
`use crate::db::get_pool;` (and `get_db_path` if its own code needs it). Do not touch
`repository` yet. Build + test must be green here.

**Stage B — repoint `repository`.**
Change `repository/agent/mod.rs::pool()` to `crate::db::get_pool().await` and the four
`app/chat_history.rs` call sites. Grep for `llm::usage::get_pool` and confirm zero
remaining hits.

**Stage C — resolve the `history` wrapper (§4).**
Either delete it (option 1) or document it as a shim (option 2).

**Stage D — documentation pass.**
Update `repository/agent/mod.rs`'s doc comment (currently explains that `pool()` calls
`crate::llm::usage::get_pool()` so the pool source can be swapped in one place) to point
at `crate::db`. Update the "Refactor #2" entry in `README.md` if one is tracked there.

## 6. Invariants that must not regress

The pool has non-obvious behaviour that a careless move will break. Preserve each one:

1. **Test isolation.** In `cfg(test)`, `get_pool()` must never open the user's live
   `assets/ragasukma.db`. It must keep using a temp WAL database with migrations applied.
2. **One pool per process.** `POOL` is a `OnceCell`; a second initialisation path must not
   be introduced (e.g. by calling `get_db_path()` eagerly at startup).
3. **Stale temp DB reaping.** The PID-based reaping (`should_reap`, `process_is_alive`,
   `reap_stale_test_dbs`) must keep running under `cfg(test)` only, with the same
   semantics.
4. **`get_db_path` precedence order** — `RAGASUKMA_DB` env var, then nearest existing
   `assets/ragasukma.db` while walking up from cwd, then `crate::assets`, then
   `config::get_app_data_dir()`. Do not reorder.
5. **`#[cfg(test)]` / `#[cfg(not(test))]` split** on `get_pool` must survive the move
   intact; a single unconditional function would silently break every test.

## 7. Verification

```bash
cargo build                      # expect 0 warnings
cargo clippy --all-targets       # expect 0 warnings
cargo test                       # expect 203 unit + 3 integration passing
cargo fmt --check                # expect clean
```

Plus two path checks that prove the cycle is actually gone:

```bash
grep -rn "llm::usage::get_pool\|crate::llm" src/repository/    # expect: no pool hits
grep -rn "crate::db" src/repository/ src/llm/                  # expect: the new edge
```

`grep -rn "crate::llm" src/repository/` will still show `goals.rs` if the evidence-ranking
helper stays where it is — that is Refactor #3's concern (see §8), not a failure of this
one.

## 8. Out of scope

Deferred, tracked separately:

- `repository` → `agent::models` (8 references) and `repository` →
  `agent::snapshot` / `agent::executor` (`projects_and_sessions.rs::delete_session`
  calls `release_session` and `forget_session_generation`). Moving the model types down is
  a larger, separate change.
- Splitting `llm/providers/antigravity/v1_adapter.rs` (1187 productive lines).
- Splitting `repository/agent/goals.rs` (1015 productive lines, 41 public functions).

## 9. Definition of done

- [ ] `src/db.rs` exists and owns `POOL`, both `get_pool` variants, `get_db_path`, and the
      test-database lifecycle.
- [ ] `src/llm/usage.rs` contains usage accounting only (no pool, no path, no temp DB).
- [ ] Zero `repository → llm::usage` references.
- [ ] All five invariants in §6 hold, verified by the test suite rather than by inspection.
- [ ] `cargo build`, `cargo clippy --all-targets`, `cargo test`, `cargo fmt --check` all green.
- [ ] Documentation updated (Stage D).
