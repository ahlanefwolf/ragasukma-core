# Refactor #3 — Split the oversized modules

**Status: PLAN (not yet executed)**

Depends on: nothing hard, but land Refactor #2 first — it touches `repository` and the
`llm` module boundary, and doing both at once makes the diffs hard to review.

## 1. Problem

Six files exceed 1000 lines. Raw line count is misleading here, because `#[cfg(test)]`
modules live inline and account for a large share of several of them. Measured split:

| File | Total | Productive | Test |
|---|---|---|---|
| `src/agent/tools/mod.rs` | 1454 | **~947** | 507 |
| `src/repository/agent/goals.rs` | 1258 | **~1015** | 243 |
| `src/agent/mcp/mod.rs` | 1201 | **~1066** | 135 |
| `src/llm/providers/antigravity/v1_adapter.rs` | 1190 | **~1187** | 3 |
| `src/agent/executor/stream.rs` | 1188 | **~826** | 362 |
| `src/agent/executor/tool_runner.rs` | 1085 | **~1038** | 47 |

Two files are genuinely oversized on productive code alone: `v1_adapter.rs` (1187) and
`tool_runner.rs` (1038). `goals.rs` (1015) is close behind. The rest are mostly large
because of inline tests.

## 2. Two separate problems, two separate fixes

The six files are not one problem. They are:

**(a) Productive code that is too big** — `v1_adapter.rs`, `tool_runner.rs`, `goals.rs`,
`mcp/mod.rs`. These need real decomposition along responsibility lines.

**(b) Inline test modules that inflate a file** — `tools/mod.rs` (507 test lines),
`stream.rs` (362), `goals.rs` (243). Moving the tests to a sibling `_tests.rs` file (the
pattern already used by `v1_adapter_tests.rs`) removes the size without touching behaviour.

Doing (b) first is cheap, low-risk, and shrinks the diff for (a). Doing (a) first means the
test move has to be redone against the new layout.

## 3. Stage 0 — extract inline tests (low risk, do first)

Pattern to follow, already present in the crate:

```rust
// src/llm/providers/antigravity/v1_adapter.rs
#[cfg(test)]
#[path = "v1_adapter_tests.rs"]
mod tests;
```

Convert these files the same way:

| File | Test lines to move | Suggested target |
|---|---|---|
| `agent/tools/mod.rs` | ~507 | `agent/tools/mod_tests.rs` |
| `agent/executor/stream.rs` | ~362 | `agent/executor/stream_tests.rs` |
| `agent/executor/tool_runner.rs` | ~47 | `agent/executor/tool_runner_tests.rs` |
| `repository/agent/goals.rs` | ~243 | `repository/agent/goals_tests.rs` |
| `agent/mcp/mod.rs` | ~135 | `agent/mcp/mod_tests.rs` |

Tests must use only the public (or `pub(crate)`) surface of the module they moved out of,
or stay in a child module so privacy is preserved. `#[path]` on a `mod tests;` declared
*inside* the original file keeps visibility identical — that is the safe form.

Verification for stage 0 is exact: same test count before and after.

```bash
cargo test 2>&1 | grep "^test result"   # 203 + 3, unchanged
```

## 4. Stage A — `v1_adapter.rs` (1187 productive, ~0 tests)

Highest priority for real splitting: it is large, almost untested, and mixes several
unrelated jobs. Current contents by concern:

| Concern | Items |
|---|---|
| Thought-signature cache | `THOUGHT_SIGNATURE_STORE`, `signature_family`, `is_signature_compatible`, `sanitize_thought_signature`, `store_thought_signature_scoped`, `store_thought_signature`, `get_thought_signature_scoped`, `get_thought_signature`, `resolve_call_signature_scoped`, `resolve_call_signature`, `is_valid_base64` |
| Model / thinking config | `resolve_model_alias`, `resolve_gemini_thinking_config`, `resolve_gemini_thinking_budget` |
| Request payload building | `build_gemini_payload`, `clean_parameters_schema`, `sanitize_schema_recursive`, `sanitize_system_prompt` |
| Streaming transport | `execute_v1_stream` (~220 lines) |

Proposed split under `src/llm/providers/antigravity/`:

```
v1_adapter/
  mod.rs                 ← re-exports; keeps `crate::...::v1_adapter::X` paths working
  signature.rs           ← thought-signature store + family compatibility
  model_config.rs        ← alias + thinking budget resolution
  payload.rs             ← build_gemini_payload + schema/system-prompt sanitising
  stream.rs              ← execute_v1_stream
v1_adapter_tests.rs      ← unchanged, still pointed at by mod.rs
```

Keep `mod.rs` re-exporting everything so call sites do not change in the same commit.

Risk: `execute_v1_stream` is the hot path for the Antigravity provider. It has almost no
tests (3 lines). **Before touching it, add a characterisation test** that captures the
request payload it produces for a fixed input, so the move is provably a no-op. Without
that, this stage is a blind refactor of untested transport code — say so rather than
pretending it is safe.

## 5. Stage B — `repository/agent/goals.rs` (1015 productive, 41 public fns)

Large but structurally simple: it is ~41 public functions, most with a `_with_pool`
variant. The natural split follows the domain, and the module already hints at it:

```
repository/agent/goals/
  mod.rs           ← re-exports + `pool()`-based wrappers
  lifecycle.rs     ← create_goal, get_goal, list_*, update_goal_phase/status, transition_goal_phase
  conclusion.rs    ← conclude_goal*, update_goal_conclusion, conclude_goal_if_verified
  evidence.rs      ← save_evidence*, link_evidence_to_goal, list_evidences_by_project,
                     get_active_evidences_for_goal, mark_evidence_stale_by_file
  verification.rs  ← record_verification*, list_goal_verifications
```

Important detail: three public functions are thin delegators with **no** `_with_pool`
variant by design (`conclude_goal`, `save_evidence`, `record_verification`). They forward to
`*_with_verification` / `*_and_link` / `*_with_status` with `None`. Do not "fix" them into
`_with_pool` functions during the split — the absence is deliberate and documented in
Refactor #1.

## 6. Stage C — `agent/executor/tool_runner.rs` (1038 productive, 47 test lines)

Near-zero tests, >1000 productive lines, and it is in the turn execution hot path. Treat
this like `v1_adapter.rs`: characterisation tests first, split second. Without tests, a
mechanical split is a `git mv` of functions with no safety net; if something subtle
(send-bound, borrow lifetime, error mapping) is disturbed, the suite will not catch it.

Recommendation: **defer** this until Stage A has shown the characterisation-test approach
works, or until `tool_runner` grows tests naturally.

## 7. Stage D — `agent/tools/mod.rs` and `agent/mcp/mod.rs` (after Stage 0)

Once inline tests are out, `tools/mod.rs` is ~947 productive lines: five free functions plus
`execute_tool_call`, whose `match` is a single ~550-line function handling eight tools
(`workspace_list_dir`, `workspace_read_file`, `workspace_write_file`, `workspace_rollback`,
`workspace_edit_file`, `workspace_grep`, `workspace_run_command`, `activate_extension`).

Per-tool dispatch functions (`fn dispatch_workspace_edit_file(...)` etc.) inside a `mod
dispatch;` would cut it substantially. The `match` becomes eight one-line forwards.

`agent/mcp/mod.rs` (~1066 productive) splits cleanly along an existing seam:
transport (`split_shell_words`, `resolve_executable`, `call_stdio_rpc`, `call_http_rpc`,
`McpClient`) vs. command-template utilities (`extract_command_template`,
`escape_shell_arg_value`, `render_template_placeholders`,
`normalize_command_for_windows`, `run_command_template`) vs. tool resolution
(`resolve_active_mcp_tools`, `parse_markdown_to_tool_def`, `call_active_mcp_tool`).

## 8. Ordering and rationale

| Order | Stage | Risk | Why here |
|---|---|---|---|
| 1 | Stage 0 — move inline tests | Low | Shrinks files for free, exact test-count check |
| 2 | Stage A — `v1_adapter` | Medium | Biggest untested file; characterisation tests add real value |
| 3 | Stage D — `mcp` split | Low–medium | Seam already exists (transport vs templates) |
| 4 | Stage D — `tools` dispatch | Medium | Hot path, but per-tool split is mechanical |
| 5 | Stage B — `goals` | Low | Mechanical, but 41 fns touched; do when diff budget allows |
| 6 | Stage C — `tool_runner` | High | Defer until tested |

## 9. Verification per stage

```bash
cargo build                  # 0 warnings
cargo clippy --all-targets   # 0 warnings
cargo test                   # 203 unit + 3 integration, unchanged count
cargo fmt --check
```

Additionally, after every stage:

```bash
wc -l src/<each touched file>    # confirm the target file is under 1000 productive
git log --follow -M --stat        # confirm history survives the move
```

Test count must be **identical** at every stage. A split that changes the count means a
test was silently dropped — that is a failure, not a rounding error.

## 10. Out of scope

- Moving `repository`'s `agent::models` dependency (Refactor #2 §8).
- Introducing new tests beyond the characterisation tests named in §4 and §6.
- Any behavioural change: every stage here is structural only.
