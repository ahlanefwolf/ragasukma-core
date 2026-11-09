# Refactor #1 — Consolidate the two `repository` layers

> Status: **PLAN (not yet executed)**
> Goal: one obvious place to put data access. No public-API break.
> Scope: `src/repository/` ⇄ `src/agent/repository/`

## 1. Problem (verified against the code)

Today there are **two** modules with the same conceptual name and the same DB:

| Location | Files | Domain |
|---|---|---|
| `src/repository/` | `chat_history.rs`, `todo.rs` | app-level entities |

> Update: the former `src/repository/skill.rs` (SQLite skill CRUD) was removed — skills are
> now discovered from `.skill` / `.skills` folders via `crate::agent::skills`. It is **not**
> part of this refactor. Remaining files to move: `chat_history.rs`, `todo.rs` + 4 agent files.
| `src/agent/repository/` | `audit.rs`, `goals.rs`, `messages.rs`, `projects_and_sessions.rs` | agent-domain entities |

Both hit the same SQLite pool. A newcomer cannot tell where a new query belongs.

### Facts collected before planning (why this is cheap to fix)

- **No name collisions** between the two public function sets (`comm -12` → empty).
- **External consumers are tiny**:
  - `src/llm/mod.rs` → `crate::repository::chat_history::log_conversation_turn`
  - `tests/evidence_bool_roundtrip.rs` → `ragasukma_core::repository::migrate_database`
- `src/agent/repository/*.rs` uniformly use `super::{generate_id, now_secs}` and `super::pool()`.
- `agent::repository::pool()` already **delegates** to `crate::llm::usage::get_pool()` — single pool, good.
- `src/repository/*` take `&SqlitePool` as an argument; `agent/repository/*` call the global `pool()`.
  *This signature asymmetry is the one real semantic difference to reconcile.*

## 2. Target layout

```
src/repository/
    mod.rs            // re-exports everything, owns pool(), now_secs(), generate_id(), migrate_database()
    app/
        chat_history.rs
        todo.rs
    agent/
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

## 3. Backwards-compat shims (so nothing breaks)

- Keep `src/agent/repository/mod.rs` as a **thin re-export alias**:
  ```rust
  pub use crate::repository::agent::*;
  ```
  → every `crate::agent::repository::foo` call site keeps working. Deprecate later.
- Keep `pub use self::repository::*;` in `agent/mod.rs` untouched.

## 4. Keep-API strategy

Because there are **no collisions** and only **2 external consumers**, we can:

1. Move files with `git mv` (preserve history/blame).
2. Rewrite internal `super::{pool, now_secs, generate_id}` paths (they all live in the new
   `src/repository/mod.rs`) — one `sed` across `agent/*` files, then compile.
3. Leave the old `agent::repository` module as an alias so *nothing outside* changes in this step.

## 5. Staged execution (each stage compiles + tests green)

| Stage | Work | Risk | Verify |
|---|---|---|---|
| **A** | `mkdir src/repository/{app,agent}`; `git mv` the 7 files; fix `mod.rs` | low | `cargo build`; `cargo test` |
| **B** | Add alias `agent::repository = pub use crate::repository::agent::*` | low | `cargo test` (all imports still resolve) |
| **C** | Fix the `&SqlitePool` vs `pool()` asymmetry (see §6) | medium | tests |
| **D** | Delete the alias, update the few call sites to `crate::repository::agent::*` | low | `cargo test` |
| **E** | Update `tests/` + docs; run `rustfmt` on touched files only | low | `cargo test` |

## 6. The one real design decision: pool source

Two conventions coexist:

```rust
// src/repository/skill.rs  (explicit pool, testable)
pub async fn get_all_skills_from_db(pool: &SqlitePool) -> Result<Vec<SkillItem>, sqlx::Error>

// src/agent/repository/goals.rs  (global pool via helper)
pub async fn get_goal(goal_id: &str) -> Result<Option<Goal>> {
    let pool = super::pool().await;
    ...
}
```

**Recommendation:** standardize on **explicit `&SqlitePool` parameters** for the pure query
functions (trivially unit-testable with an in-memory/temp pool — exactly what the new
integration tests want), and keep `pool()` only as a convenience wrapper used by callers
that legitimately own no pool. Do this **after** stage B, as its own PR, so the move and the
signature change don't land together.

## 7. Non-goals (of this refactor)

- Not touching `llm::usage::get_pool()` location yet (that's refactor **#2**).
- Not splitting files >1000 lines (that's **#3**).
- No behaviour change. This is a pure move + re-export.

## 8. Definition of done

- `cargo test` green (unit + `tests/evidence_bool_roundtrip.rs`).
- `cargo build` with **zero** new warnings.
- Only one directory named `repository` in `src/` (the alias aside).
- `git log --follow` still tracks moved files (i.e. `git mv` was used).
