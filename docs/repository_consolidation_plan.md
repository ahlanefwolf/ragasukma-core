# Refactor #1 — Consolidate the two `repository` layers

> Status: **EXECUTED (stages A–E complete, all verification green)**
> Goal: one obvious place to put data access. No public-API break.
> Scope: `src/repository/` ⇄ `src/agent/repository/` (the latter no longer exists)

## 0. Outcome (verified against the code)

| Definition of done (§8) | Result |
|---|---|
| `cargo test` green | ✅ 203 unit + 3 integration (`tests/evidence_bool_roundtrip.rs`), 0 failed |
| `cargo build` with zero new warnings | ✅ 0 warnings (`cargo build` and `cargo clippy --all-targets`) |
| Only one directory named `repository` in `src/` | ✅ `find src -type d -name repository` → `src/repository` only |
| `git log --follow` still tracks moved files | ✅ e.g. `audit.rs` traces to `af861f3`, `todo.rs` to `f55986c` |

Stage → commit map (oldest first):

| Stage | Commit | Work |
|---|---|---|
| **A** | `df32d2f` | `git mv` all files into `src/repository/{app,agent}/`; rewire `mod.rs` |
| **D** | `1c0c9c9` | Deleted the `agent::repository` shim; call sites point at `crate::repository::agent::*` |
| **C1** | `4452130` | `_with_pool` cores for `audit.rs`, `messages.rs` |
| **C2** | `763709f` | `_with_pool` cores for `goals.rs` |
| **C3** | `472b718` | `_with_pool` cores for `projects_and_sessions.rs` |
| **E** | `4dff4d7` | `cargo fmt` across crate + doc refresh |
| — | `64c3720` | Clippy clean-up (redundant closure, `assert_eq!(_, true)`) |

Stage B (the temporary alias) was created and then removed by `1c0c9c9`; it leaves no trace
in the tree by design.

## 1. Problem (as originally verified)

Two modules shared the same conceptual name and the same DB:

| Location | Files | Domain |
|---|---|---|
| `src/repository/` | `chat_history.rs`, `todo.rs` | app-level entities |
| `src/agent/repository/` | `audit.rs`, `goals.rs`, `messages.rs`, `projects_and_sessions.rs` | agent-domain entities |

Both hit the same SQLite pool. A newcomer could not tell where a new query belonged.

> Note: the former `src/repository/skill.rs` (SQLite skill CRUD) was removed before this
> refactor — skills are now discovered from `.skill` / `.skills` folders via
> `crate::agent::skills`. It was never part of this work.

### Facts collected before planning (why this was cheap to fix)

- **No name collisions** between the two public function sets (`comm -12` → empty).
- **External consumers were tiny**:
  - `src/llm/mod.rs` → `crate::repository::chat_history::log_conversation_turn`
  - `tests/evidence_bool_roundtrip.rs` → `ragasukma_core::repository::migrate_database`
- `src/agent/repository/*.rs` uniformly used `super::{generate_id, now_secs}` and `super::pool()`.
- `agent::repository::pool()` already **delegated** to `crate::llm::usage::get_pool()` — single
  pool, good.
- `src/repository/*` took `&SqlitePool` as an argument; `agent/repository/*` called the global
  `pool()`. *This signature asymmetry was the one real semantic difference to reconcile.*

## 2. Target layout (as built)

```
src/repository/
    mod.rs            // re-exports everything, owns pool(), now_secs(), generate_id(), migrate_database()
    app/
        mod.rs
        chat_history.rs
        todo.rs
    agent/
        mod.rs
        audit.rs
        goals.rs
        messages.rs
        projects_and_sessions.rs
```

`mod.rs` re-exports so that **all existing paths keep compiling**:

```rust
pub mod app;      // was: src/repository/*.rs
pub mod agent;    // was: src/agent/repository/*.rs

pub use app::{ chat_history, todo };
pub use app::chat_history::*;
pub use app::todo::*;
pub use agent::{ audit, goals, messages, projects_and_sessions };
```

## 3. Backwards-compat shim (transient)

During stages B–C, `src/agent/repository/mod.rs` was a thin alias:

```rust
pub use crate::repository::agent::*;
```

Stage D (`1c0c9c9`) deleted it and rewrote call sites to `crate::repository::agent::*`.
The shim is gone; do not reintroduce it.

## 4. Keep-API strategy (as executed)

1. Moved files with `git mv` (history/blame preserved — see `git log --follow`).
2. Rewrote internal `super::{pool, now_secs, generate_id}` paths to point at the new
   `src/repository/mod.rs`.
3. Kept the old `agent::repository` module as an alias during the move, then removed it once
   call sites were updated.

## 5. Staged execution

| Stage | Work | Risk | Outcome |
|---|---|---|---|
| **A** | `mkdir src/repository/{app,agent}`; `git mv` the 7 files; fix `mod.rs` | low | ✅ `df32d2f` |
| **B** | Add alias `agent::repository = pub use crate::repository::agent::*` | low | ✅ later removed in D |
| **C** | Fix the `&SqlitePool` vs `pool()` asymmetry (see §6) | medium | ✅ `4452130`, `763709f`, `472b718` |
| **D** | Delete the alias, update the few call sites to `crate::repository::agent::*` | low | ✅ `1c0c9c9` |
| **E** | Update `tests/` + docs; `cargo fmt` | low | ✅ `4dff4d7` |

## 6. The one real design decision: pool source

Two conventions coexisted:

```rust
// src/repository/app/todo.rs  (explicit pool, testable)
pub async fn get_all_todos_from_db(pool: &SqlitePool) -> Result<Vec<TodoItem>, sqlx::Error>

// src/repository/agent/goals.rs  (global pool via helper)
pub async fn get_goal(goal_id: &str) -> Result<Option<Goal>> {
    let pool = pool().await;
    ...
}
```

**Decision (implemented):** standardize on **explicit `&SqlitePool` parameters** for the pure
query functions, and keep `pool()` only as a convenience wrapper for callers that legitimately
own no pool.

Implemented as paired functions: a `*_with_pool` core holds the query, and the legacy
convenience signature delegates to it. Example:

```rust
pub async fn get_goal(goal_id: &str) -> Result<Option<Goal>> {
    get_goal_with_pool(pool().await, goal_id).await
}
```

Coverage at time of writing:

| File | Public fns | With `_with_pool` core |
|---|---|---|
| `agent/audit.rs` | 8 | 8 ✅ |
| `agent/messages.rs` | 8 | 8 ✅ |
| `agent/projects_and_sessions.rs` | 26 | 26 ✅ |
| `agent/goals.rs` | 41 | 38 ✅ |

The three `goals.rs` functions without their own `_with_pool` core — `conclude_goal`,
`save_evidence`, `record_verification` — are thin delegators that perform **no DB access**;
they forward straight to a sibling that already has a pool-injectable core. Adding aliases for
them would add surface without value.

Also fixed along the way: `set_project_mcp_servers_with_pool` no longer calls the global
wrapper from inside its `BEGIN IMMEDIATE` transaction, removing a pool re-entry hazard.

## 7. Non-goals (of this refactor)

- Not touching `llm::usage::get_pool()` location yet (that's refactor **#2**).
- Not splitting files >1000 lines (that's **#3**).
- No behaviour change. This was a pure move + re-export, plus pool-parameter plumbing.

## 8. Definition of done

- `cargo test` green (unit + `tests/evidence_bool_roundtrip.rs`). ✅
- `cargo build` with **zero** new warnings. ✅
- Only one directory named `repository` in `src/`. ✅
- `git log --follow` still tracks moved files (i.e. `git mv` was used). ✅
