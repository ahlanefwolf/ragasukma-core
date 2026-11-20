# Autonomous Agent Harness Improvement Roadmap

## Purpose

`ragasukma-core` is a sequential, goal-aware, tool-using autonomous agent harness. Its current architecture is a strong foundation for local coding and project assistance, but production reliability depends on more than the model loop itself.

This document records the recommended improvements for making the engine safer, more recoverable, easier to observe, and easier to embed in TUI, web, API, or worker applications.

## Current assessment

The core is already relatively agnostic to natural-language input and model provider details. It has a bounded sequential execution loop, session and goal state, tool execution, MCP integration, verification, evidence handling, cancellation, and context management.

The main remaining risks are not a missing orchestration layer. They are reliability at process boundaries, durable recovery, security enforcement, consistent error semantics, and behavioral evaluation.

## Priority 1: Reliability and recovery

### 1. Standardize error handling

Every layer should distinguish between:

- **Fatal errors**: the turn must stop.
- **Recoverable errors**: the operation may be retried or the agent may continue with an explicit warning.
- **Optional data missing**: a default is acceptable, but the fallback should be observable.

Avoid silently discarding important failures from database updates, session state changes, MCP lifecycle updates, and verification history. Prefer domain error types with stable categories over unstructured strings where the public API permits it.

### 2. Add a durable turn lifecycle

A session lifecycle is not enough for recovery of an autonomous turn. Introduce a durable execution or `turn_id` and explicit states such as:

```text
queued -> running -> waiting_tool -> observing -> verifying -> completed
                                      \-> failed
                                      \-> cancelled
```

Each state transition should record the actor/event, timestamp, iteration, and relevant error category. Startup recovery should identify stale `running` or `waiting_tool` turns and either resume them safely or mark them interrupted according to a documented policy.

### 3. Add classified retry policies

Retry decisions must depend on the failure class rather than retrying every error:

| Failure | Recommended policy |
| --- | --- |
| HTTP 429 | Retry with bounded exponential backoff |
| HTTP 5xx | Retry a limited number of times |
| Provider timeout | Retry within the turn budget |
| Invalid tool arguments | Do not blindly retry; let the model correct the call |
| Permission denied | Do not retry automatically |
| MCP process exit | Optionally restart once, then fail clearly |
| Database lock | Retry briefly, then surface the failure |

Retries must count against a bounded budget and emit observable events.

## Priority 2: Security and policy enforcement

### 4. Enforce filesystem boundaries

All file operations must remain inside the configured workspace boundaries. Validate normalized paths, reject traversal, and account for symlink escapes. The policy must be enforced by runtime tools, not only described in the system prompt.

### 5. Add command execution policy

Commands should have an explicit risk policy. Depending on the host application, support:

- read-only mode;
- approval-required mode;
- command allowlists or project policies;
- timeouts and output limits;
- process/resource limits;
- an audit record for every command.

### 6. Treat project content as untrusted input

README files, source files, issues, MCP output, and tool output may contain prompt-injection instructions. They must be treated as data and never override system, developer, security, or runtime policies. Add tests where malicious instructions appear in inspected project content.

## Priority 3: Architecture and observability

### 7. Make planner, executor, observer, verifier, and goal management boundaries explicit

The conceptual responsibilities should remain separable:

- **Planner** selects the next action.
- **Executor** runs a tool.
- **Observer** converts tool output into model-visible observations.
- **Verifier** checks whether the goal or change succeeded.
- **Goal manager** owns phase and completion transitions.

Clear boundaries improve testing, replay, read-only mode, human approval, and future strategy changes.

### 8. Use a structured event contract

Define stable events such as:

```text
TurnStarted
ModelRequested
ToolRequested
ToolCompleted
VerificationStarted
VerificationCompleted
TurnCompleted
TurnFailed
TurnCancelled
```

Events should include `session_id`, `turn_id`, timestamp, iteration, tool name where applicable, duration, error category, and token/cost metadata when available. Structured events should be the primary source for UI progress and diagnostics rather than parsing log text.

### 9. Strengthen the public API

Prefer domain types over interchangeable strings:

```rust
pub struct AgentRequest {
    pub session_id: SessionId,
    pub message: UserMessage,
    pub model: Option<ModelSelection>,
    pub mode: ExecutionMode,
}
```

Useful public concepts include `SessionId`, `GoalId`, `TurnId`, `ModelId`, `ToolName`, `ExecutionMode`, `TurnOutcome`, and `AgentError`. Host applications should submit messages and consume events without depending on provider-specific internals.

## Priority 4: Quality and evaluation

### 10. Add autonomous behavior benchmarks

Unit tests validate internal logic, but autonomous behavior needs scenario-based evaluation. Include scenarios for:

- fixing a failing test;
- inspecting an unfamiliar project;
- multi-file changes;
- ambiguous requests;
- read-only analysis;
- prompt injection in a project file;
- provider timeout;
- MCP process failure;
- a change that fails verification.

Track completion rate, verification success, tool-call count, iteration count, unnecessary reads, recovery rate, and unsafe-action rate.

### 11. Expand concurrency and cancellation tests

Test two turns on one session, independent sessions, cancellation during tool execution, cancellation during verification, MCP exit during a request, database locks, notifications before responses, out-of-order responses, and restart after an interrupted state.

### 12. Improve context and cost management

Continue improving context compression with durable goal summaries, deduplicated file content, cache invalidation, provider-specific budgets, and token/cost accounting per session and goal.

## Recommended implementation order

### Phase 1: Reliability

1. Standardize domain error categories.
2. Remove silent failures from important state transitions.
3. Add durable `turn_id` and turn status.
4. Add stale-turn recovery.
5. Add classified retry and backoff.

### Phase 2: Security

1. Harden normalized path and symlink validation.
2. Add command risk policies and approval mode.
3. Add tool-call audit records.
4. Add prompt-injection regression tests.

### Phase 3: Quality

1. Formalize planner/executor/verifier boundaries.
2. Publish a structured event schema.
3. Add autonomous behavior benchmarks.
4. Expand concurrency and cancellation coverage.
5. Document the embedding/public API contract.

## Success criteria

The harness should be considered substantially hardened when it can:

- recover or clearly terminate an interrupted turn;
- classify and expose failures without silently losing important state;
- enforce workspace and command policies at runtime;
- survive bounded provider and MCP failures;
- provide a complete structured event trail;
- demonstrate goal completion and safety through scenario benchmarks;
- support multiple host interfaces without exposing provider-specific implementation details.
