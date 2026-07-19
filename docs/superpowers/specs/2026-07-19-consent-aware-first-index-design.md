# Consent-Aware First Index Design

## Summary

Repository registration, user approval, runtime initialization, and indexing are currently conflated. A repository can be registered and reported as `indexing_started` even though the server only opens its stores and starts a watcher. No cold scan runs until a relevant filesystem event or a manual refresh occurs.

This design introduces a durable, one-time first-index permission gate. Every binding source must obtain explicit user approval before the repository's first successful full index. Approval starts a real background `InitialBind` job. Once that first index succeeds, normal tool dispatch and automatic watcher reindexing continue without further permission prompts.

## Goals

- Require explicit user approval before the first full index for every repository, including explicit `?repo=` and `bind_workspace` bindings.
- Make MCP instructions and tool responses tell the calling model to ask the user in chat before approving the first index.
- Start a real full index immediately after approval, without waiting for a filesystem event.
- Persist approval so a failed or interrupted index can retry without asking the user again.
- Persist completion so later sessions and daemon restarts recognize the repository as ready.
- Deduplicate concurrent approval and tool-call races so only one first-index job runs per repository.
- Keep later watcher updates and deliberate reindexes automatic after the first successful index.
- Preserve compatibility with repositories indexed by older server versions.
- Preserve `INDEX_CONSENT_REQUIRED=false` as an operator-controlled opt-out for CI and benchmarks.

## Non-Goals

- Asking for permission before every watcher update or later manual reindex.
- Adding a new dashboard layout.
- Changing file selection, parsing, ranking, embedding, or retrieval behavior.
- Making optional external-index producer success a requirement for first-index completion.
- Allowing the MCP server to contact the user directly. The server supplies an actionable response; the calling model performs the chat interaction.

## Existing Behavior and Root Cause

`SessionManager::get_or_create_repo` canonicalizes and registers the path, initializes SQLite, Tantivy, LanceDB, the indexer, and the retriever, then starts the file watcher. It does not call `IndexPipeline::index_all`.

The complete full-index call-site set currently consists of watcher-triggered indexing and manual refresh. The watcher only wakes after a filesystem event. Consequently, a newly registered repository can remain empty indefinitely while consent handlers return `indexing_started`.

The existing `IndexConsent::Approved` value is not sufficient proof of user approval. Older code writes `Approved` whenever `RepoRegistry::register` is called, including paths that were registered without a chat confirmation. Migration must therefore distinguish old automatic approval from new explicit first-index authorization.

## Lifecycle Model

Add two optional timestamps to each `RepoEntry`, both using Serde defaults for backward-compatible registry loading:

```rust
pub initial_index_approved_at: Option<String>,
pub initial_index_completed_at: Option<String>,
```

The effective lifecycle is derived in this order:

1. **Declined**: `consent == IndexConsent::Declined`.
2. **Ready**: `initial_index_completed_at` is present, or the repository database contains a successful persisted index run.
3. **Approved pending**: `initial_index_approved_at` is present but no successful run exists.
4. **Needs approval**: none of the conditions above apply.

`IndexConsent::Approved` alone does not satisfy the first-index permission gate. This ensures legacy registered-but-unindexed repositories ask once instead of silently indexing.

For legacy repositories, a successful persisted index run is authoritative. The server treats the repository as ready and lazily backfills `initial_index_completed_at`. This includes repositories with zero symbols because index-run persistence, rather than symbol count, proves that a full scan completed.

Approval writes both `consent = Approved` and `initial_index_approved_at`. A successful first-index job writes `initial_index_completed_at`. Declining writes `consent = Declined` and clears any first-index approval that has not already produced a completed index.

Registration itself no longer grants first-index authorization. Dashboard Add may create the registry entry and data-directory identity, but it leaves both lifecycle timestamps empty.

## Repository Binding and Tool Dispatch

All binding sources use the same lifecycle gate:

- `?repo=` URL query
- MCP `roots/list`
- `bind_workspace`
- Single-repository registry fallback
- Dashboard Add followed by MCP use

Project-marker and unsafe-root checks remain unchanged. The permission rule is independent of whether the binding is explicit or implicit.

The default `INDEX_CONSENT_REQUIRED=true` applies this gate to every binding source. When an operator explicitly sets it to `false`, the coordinator records authorization and starts the real first-index job without a chat prompt. The opt-out changes permission handling only; it does not restore the old watcher-dependent cold-start behavior.

`bind_workspace` records the session binding, evaluates the lifecycle, and returns the relevant lifecycle payload. It must not prewarm an unapproved repository. URL and roots-based bindings are evaluated by `resolve_state` on the first tool call.

Normal tool dispatch receives one of these outcomes:

- `Ready(AppState)`: dispatch the requested tool.
- `ConsentRequired(Value)`: return an actionable `consent_required` result.
- `Indexing(Value)`: return `indexing_started` or `indexing_in_progress` without dispatching against a partial index.
- `Declined(Value)`: return the existing declined response.

`approve_indexing` remains callable before normal state resolution. It is the only MCP action that converts a pending first index into an approved one.

## Model-Facing Contract

The server instructions and the `consent_required` payload must state all of the following explicitly:

- The repository has no completed index.
- The model must tell the user in chat that the repository needs its first full index and that indexing uses local compute, memory, and disk.
- The model must wait for explicit user approval.
- Only after approval may the model call `approve_indexing` with `decision: "approve"`.
- If the user declines, the model calls `approve_indexing` with `decision: "decline"`.
- The model must not infer approval from the user merely opening or binding a repository.

Lifecycle response shapes are:

```json
{
  "status": "consent_required",
  "repo": "/absolute/path",
  "repo_id": "0123456789abcdef",
  "message": "This repository has no completed index and needs its first full index.",
  "action": "Tell the user in chat that indexing uses local compute, memory, and disk. Ask for permission. After an explicit yes, call approve_indexing."
}
```

```json
{
  "status": "indexing_started",
  "repo": "/absolute/path",
  "repo_id": "0123456789abcdef",
  "job_id": "initial-0123456789abcdef-...",
  "message": "First index started after user approval. Tell the user indexing is in progress."
}
```

```json
{
  "status": "indexing_in_progress",
  "repo": "/absolute/path",
  "repo_id": "0123456789abcdef",
  "job_id": "initial-0123456789abcdef-...",
  "message": "First index is still running. Tell the user and retry the code request later."
}
```

The existing declined response remains available. Ready repositories do not emit lifecycle payloads and their tools behave normally.

## Initial Index Coordinator

Introduce a repository-scoped coordinator owned by `SessionManager`. It is responsible for:

- Deriving the durable lifecycle from the registry and persisted index-run evidence.
- Serializing approval and startup decisions with a per-repository async lock.
- Detecting an existing running `InitialBind` job.
- Creating or reusing the per-repository `AppState` only after approval or readiness.
- Starting exactly one background first-index job.
- Marking the job succeeded or failed in `JobRegistry`.
- Persisting completion only after the native index succeeds.
- Starting the file watcher only after completion.

The background job runs `index_all_with_external_index` using a dedicated `InitialBind` trigger. Native indexing runs first. External producer execution follows the same eligibility as an explicit user-triggered refresh, but producer failure remains metadata and does not prevent first-index completion.

If approval is persisted but no job is running and no successful run exists, the next binding or tool call starts a replacement job automatically. It returns `indexing_started` and does not ask the user again.

If a job is already running, concurrent calls return `indexing_in_progress` with the existing job ID. They do not start another task.

## Watcher Interaction

The watcher must not start when an unapproved or approved-but-incomplete repository state is created. This prevents a filesystem event from bypassing consent or racing the initial full index.

After the first successful native index, watcher startup is idempotent. Later events continue to use the existing debounce, rate limiting, full scan, fingerprint skipping, and `WatchReindex` job behavior. They never require another permission prompt.

Warm-cache eviction cancels the watcher as it does today. Reopening a ready repository recreates its state and restarts the watcher without asking again.

## Failure and Restart Semantics

- State initialization failure leaves approval persisted and returns an index failure result.
- Native indexing failure marks the job failed and leaves completion unset.
- Optional external producer failure does not fail the first index.
- A daemon restart loses transient job state but retains approval. The next access starts a replacement job automatically.
- If the native index completed and persisted its run before a crash, that run is authoritative even if the registry completion timestamp was not written.
- Repeated approval calls are idempotent. They return ready, in-progress, or newly-started status as appropriate.
- An empty repository becomes ready after a successful full scan because completion is based on the persisted index run.

## HTTP API and Dashboard Behavior

`POST /api/repos` registers a pending repository and makes it visible to the existing consent view. It does not create an `AppState`, start a watcher, or start indexing.

`POST /api/consent` uses the same initial-index coordinator as the MCP `approve_indexing` tool. Approval returns the background job ID. Decline records the decision without initializing repository stores.

`POST /api/repos/{id}/reindex` continues to work without a new prompt for ready repositories. For repositories that have never completed an index, it returns a consent-required response instead of bypassing the first-index gate.

Existing job and repository activity endpoints expose the `InitialBind` job. No new dashboard layout is required.

## Testing Strategy

Implementation follows test-driven development. Tests must first reproduce the current gap, then verify each lifecycle transition.

### Registry and lifecycle unit tests

- New registrations have no first-index approval or completion timestamps.
- User approval persists independently from completion.
- Successful completion persists and survives registry reload.
- Legacy entries with a successful index run resolve as ready.
- Legacy entries without a successful run resolve as needs approval even if their old consent value is `Approved`.
- Empty repositories resolve as ready after a successful persisted index run.
- Decline prevents authorization and runtime initialization.

### Session and coordinator tests

- Approval starts a real full index without a filesystem event.
- Concurrent approvals and tool calls create one `InitialBind` job.
- Calls during the job return `indexing_in_progress`.
- Native failure retains approval and a later call starts one retry.
- Simulated restart retains approval and resumes without another prompt.
- Watcher startup occurs only after first-index success and remains idempotent.
- Ready repositories use the warm cache and restart their watcher after eviction.

### MCP and API behavior tests

- URL, roots, `bind_workspace`, and single-repository binding all require approval for an unindexed repository.
- `consent_required` and server instructions explicitly tell the model to ask the user in chat and wait for a yes.
- `approve_indexing` returns a job ID and the job produces indexed symbols using the hash embedding backend.
- Successful completion unlocks normal search and navigation tools.
- Dashboard Add creates a pending repository visible to the consent endpoint.
- Dashboard approval starts the same coordinator job.
- Manual reindex of a ready repository remains permission-free.
- Manual reindex of an unready repository cannot bypass consent.

## Acceptance Criteria

- A newly bound repository never indexes before explicit first-index approval.
- The first attempted code tool call returns instructions that cause the model to ask the user in chat.
- Approval starts a full index immediately and returns a trackable job ID.
- No filesystem edit is needed to start the first index.
- Only one first-index job can run per repository.
- Tool calls cannot observe or query a partial first index.
- Approval survives failure and daemon restart without another user prompt.
- Successful first-index completion survives daemon restart.
- Existing indexed repositories remain usable without a new prompt.
- Watcher and later manual reindex behavior remain automatic after first-index completion.
