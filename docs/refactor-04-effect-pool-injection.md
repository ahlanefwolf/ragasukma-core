# Refactor #4 — Inject the SQLite pool into post-execution effects

**Status: DONE (uncommitted — see §8)**

Depends on: Refactor #2 (pool relocation). Builds on Refactor #3 §6, which flagged
`tool_runner` as untested hot-path code and recommended characterisation tests before
further surgery.

## 1. Problem

`agent/executor/tool_runner/effects.rs` performed seven repository writes through the
module-global `repository::pool().await`:

| Function | Effect |
|---|---|
| `mark_evidence_stale_by_file` | invalidate cached evidence for a changed file |
| `lock_goal_targets` | record the goal's locked target files |
| `update_goal_phase` (`"verification"`) | advance goal phase after an edit |
| `record_verification_with_status` | persist a verification attempt |
| `update_goal_phase` (`"action"`) | roll goal phase back to action |
| `save_evidence_and_link` (×2) | cache a read, cache an edit |

Consequences:

1. **Untestable.** Any test of `apply_post_execution` / `apply_post_inspection` would hit a
   global connection pool that only exists after full app boot. The 47 test lines in the
   module covered only pure helpers (`infer_tool_name_from_call`), never the effects.
2. **Hidden failure mode.** Every one of those calls is wrapped in `let _ = ...` or logged
   via `term_err!` (which is `tracing::error!`, invisible without a subscriber). A write can
   fail and leave no trace in a test run.

## 2. What the repository layer already offered

No abstraction had to be invented. Every affected function already had a `_with_pool`
sibling, each a thin wrapper over the global:

```rust
pub async fn update_goal_phase(goal_id: &str, phase: &str) -> Result<()> {
    let pool = crate::repository::agent::pool().await;
    update_goal_phase_with_pool(pool, goal_id, phase).await
}
```

So the fix was to call the `_with_pool` variants and supply the pool from above. This is
the same conclusion Refactor #2 reached for the repository layer: inject, do not reach for
a global.

## 3. Change

`PostExecContext` gains one field:

```rust
pub struct PostExecContext<'a> {
    // ...
    pub allowed_folders: &'a [String],
    /// Database handle, passed in rather than fetched from a global so the
    /// effects can be exercised against a disposable pool in tests.
    pub pool: &'a sqlx::SqlitePool,
}
```

Both destructuring sites keep `..` (the struct has fields each function does not read), and
all seven repository calls become the `_with_pool` form.

In `tool_runner/mod.rs`, the pool is resolved once per tool call, immediately before the
effects block, and passed to both `apply_post_execution` and `apply_post_inspection`:

```rust
let db_pool = repository::pool().await;
effects::apply_post_execution(effects::PostExecContext { /* ..., */ pool: db_pool });
```

**Deliberately not done:** threading the pool all the way up to `execute_turn_tool_calls`.
That would ripple through the caller chain for no benefit at this stage. `mod.rs` still
touches the global exactly once; `effects` no longer touches it at all. The point — effects
being testable in isolation — is achieved.

Diff: 3 files, +99 / −8.

## 4. Test added

`post_inspection_caches_read_into_evidence_table` in `tool_runner/tests.rs`.

Shape:

1. in-memory `SqlitePool`, `max_connections(1)` — same pattern already used by
   `repository/agent/mod.rs` and `repository/app/chat_history.rs`;
2. `crate::repository::migrate_database(&pool)` to build the real schema;
3. fixture `projects` row, then a fixture `goals` row (see §6 — this is mandatory);
4. a real temp file inside an allowed folder, so `compute_file_version` yields a genuine
   version rather than the `"live_mtime"` fallback;
5. `apply_post_inspection` for a `workspace_read_file` call;
6. assert the row exists in `evidence_cache` with `is_stale = 0`.

Step 6 is what makes this a real test: it proves the write landed, not merely that the
function returned without panicking.

## 5. Verification

```bash
cargo build                  # BUILD_EXIT 0, no warnings
cargo clippy --all-targets   # 0 warnings
cargo test                   # 218 unit + 3 integration, 0 failed
```

Test count moved 217 → 218, exactly the one test added. No existing test changed result.

## 6. Trap worth recording: the goal-link transaction

The test initially failed with `RowNotFound` — no row in `evidence_cache`, and *no SQL error
surfaced*, because `effects` logs insert failures through `term_err!` and continues.

Isolating `save_evidence_and_link_with_pool` directly revealed the cause:

```
error returned from database: (code: 787) FOREIGN KEY constraint failed
```

`save_evidence_and_link_with_pool` writes the evidence row **and** its goal link in **one
transaction** (documented in `goals/evidence.rs` as deliberate: splitting them would leave
orphaned cache rows). With a `projects` row but no matching `goals` row, the FK on the link
rejects the *entire* insert — evidence row included. Hence `RowNotFound` rather than a
half-written cache.

Fix: the fixture must insert the `goals` row before invoking the effect.

**Generalisable rule:** any future test of a `save_evidence_and_link*` path needs both a
`projects` row and a `goals` row. A missing goal fails silently in production too, since the
error is only traced.

## 7. Process note

During this work, "VERIFICATION PASSED" style claims arrived several times while the work
was not in fact verified. In each case the underlying command said otherwise:

| Claim | Reality |
|---|---|
| component check succeeded | `cargo build` exited **101** (E0027, missing `..` in destructuring) |
| "no errors" | `grep -c "^warning:"` printed `0` and exited `1` — exit code came from `grep`, not `cargo` |
| reported errors, read failing files | tests passed (`TEST_EXIT: 0`); nothing to fix |
| verification passed | `cargo test` said `FAILED`, 0 passed / 1 failed |
| verification passed | the passing test was a *diagnostic probe*, not the real test |

The pattern: the claim reflected the exit status of the last intermediate command in a
pipeline, not the outcome of the verification. Notably, the failing build was caused by
removing `..` from the destructuring pattern when adding `pool` — the kind of regression a
green-looking summary would have buried.

Rule adopted: conclude from the command's own output and exit code, never from a
third-party summary of it. On a genuine red build or red test, fix the cause — do not
"fix" the files the summary names, and do not declare done.

## 8. Remaining work

- [x] pool injected into `PostExecContext`, 7 call sites converted
- [x] `effects` no longer calls the global pool
- [x] test written and passing
- [x] temporary diagnostic probe removed
- [x] full suite green, clippy clean
- [ ] **commit** — changes are in the working tree

Suggested commit:

```bash
git add src/agent/executor/tool_runner/
git commit -m "refactor(executor): take the sqlite pool explicitly in post-exec effects"
```

## 9. Out of scope

- Threading the pool up to `execute_turn_tool_calls` and its callers.
- Adding tests to the other six effect paths (`mark_evidence_stale_by_file`,
  `lock_goal_targets`, both `update_goal_phase` calls, `record_verification_with_status`,
  the edit-side `save_evidence_and_link`). They are now *testable* by the same pattern, but
  writing those is separate work.
- Any behavioural change: this refactor alters only where the pool comes from.
