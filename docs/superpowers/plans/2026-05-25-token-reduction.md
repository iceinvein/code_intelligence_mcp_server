# Token Reduction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reduce code-intelligence benchmark token usage while preserving evidence quality and answer accuracy.

**Architecture:** First tighten the instruction contract so agents stop reading files after complete evidence. Then make the `ask_code` evidence-only response explicitly compact and self-sufficient without removing `pack.rows`, `evidence[]`, line ranges, or source bodies.

**Tech Stack:** Python benchmark harness tests with `unittest`; Rust MCP tool descriptions and `serde_json::Value` response tests.

---

## Files

- Modify: `scripts/bench_agent_qa.py`
  - Strengthen `CODE_INTEL_SYSTEM_PROMPT_EXTRA`.
- Modify: `scripts/agent_qa/tests/test_bench_agent_qa.py`
  - Assert the benchmark prompt includes the complete-coverage no-fallback contract.
- Modify: `src/tools/mod.rs`
  - Strengthen `ask_code` and `investigate` descriptions.
  - Extend existing description tests.
- Modify: `src/handlers/ask_code.rs`
  - Add a compact evidence response contract marker.
  - Tighten follow-up guidance for complete coverage.
  - Add tests for required compact fields and fallback guidance.
- Modify: `src/storage/tantivy.rs`
  - Make natural-language keyword search tolerant of punctuation that Tantivy's strict parser treats as query syntax.

## Task 1: Benchmark Prompt Contract

- [ ] **Step 1: Write the failing benchmark prompt test**

Add these assertions to `test_code_intel_toolset_instructs_and_allows_mcp_tools` in `scripts/agent_qa/tests/test_bench_agent_qa.py`:

```python
self.assertIn("coverage is complete", prompt)
self.assertIn("do not call Read, Grep, or Glob", prompt)
self.assertIn("missing source bodies", prompt)
```

- [ ] **Step 2: Run the test and verify it fails**

Run:

```bash
python3 -m unittest scripts.agent_qa.tests.test_bench_agent_qa.BenchAgentQaToolsetTests.test_code_intel_toolset_instructs_and_allows_mcp_tools
```

Expected: fail because the current prompt does not contain the complete-coverage no-fallback rule.

- [ ] **Step 3: Update `CODE_INTEL_SYSTEM_PROMPT_EXTRA`**

Replace the current final sentence in `scripts/bench_agent_qa.py` with explicit complete-coverage handling:

```python
"When `ask_code` or `investigate` returns `pack.rows`, synthesize from those "
"rows and respect `pack.coverage.status` and `role=\"candidate\"`. If "
"`pack.coverage.status` is complete and `evidence[]` or `pack.rows` contains "
"the needed line-level source, do not call Read, Grep, or Glob to re-check the "
"same files. Fall back to Read/Grep/Glob only for partial/no_hits coverage, "
"candidate rows, or missing source bodies needed for citation."
```

- [ ] **Step 4: Run benchmark prompt tests**

Run:

```bash
python3 -m unittest scripts.agent_qa.tests.test_bench_agent_qa
```

Expected: all tests pass.

## Task 2: MCP Tool Description Contract

- [ ] **Step 1: Write failing Rust description assertions**

In `src/tools/mod.rs`, extend `investigate_description_mentions_evidence_packs` and `ask_code_description_mentions_evidence_packs` with assertions that each description contains:

```rust
"coverage is complete"
"Do not call Grep/Read"
"missing source bodies"
```

- [ ] **Step 2: Run the focused Rust tests and verify failure**

Run:

```bash
cargo test -q tools::tests::investigate_description_mentions_evidence_packs
cargo test -q tools::tests::ask_code_description_mentions_evidence_packs
```

Expected: fail because descriptions do not yet include the stronger contract.

- [ ] **Step 3: Update `investigate` description**

In `src/tools/mod.rs`, revise the `InvestigateTool` description to preserve existing wording and add:

```text
When coverage is complete and rows or verified locations include the needed line-level source, Do not call Grep/Read to re-check the same files. Use Grep/Read only for partial/no_hits coverage, candidate rows, or missing source bodies needed for citation.
```

- [ ] **Step 4: Update `ask_code` description**

In `src/tools/mod.rs`, revise the `AskCodeTool` description to preserve existing wording and add:

```text
When coverage is complete and `evidence[]` or `pack.rows` includes the needed line-level source, Do not call Grep/Read to re-check the same files. Use follow-up tools only for partial/no_hits coverage, candidate rows, or missing source bodies needed for citation.
```

- [ ] **Step 5: Run focused Rust description tests**

Run:

```bash
cargo test -q tools::tests::investigate_description_mentions_evidence_packs
cargo test -q tools::tests::ask_code_description_mentions_evidence_packs
```

Expected: both tests pass.

## Task 3: Compact `ask_code` Evidence Response Contract

- [ ] **Step 1: Write failing `ask_code` compact response test**

Add a test in `src/handlers/ask_code.rs` near existing `build_evidence_only_response` tests:

```rust
#[test]
fn evidence_only_response_marks_compact_and_preserves_grounding_fields() {
    let investigate_response = json!({
        "mode_used": "discover",
        "pack": {
            "coverage": {"status": "complete"},
            "rows": [{
                "role": "verified",
                "file_path": "src/lib.rs",
                "line_start": 10,
                "line_end": 12,
                "body": "fn answer() {}"
            }]
        }
    });
    let evidence = vec![EvidenceItem {
        symbol_id: Some("src/lib.rs::answer".to_string()),
        symbol_name: Some("answer".to_string()),
        file_path: "src/lib.rs".to_string(),
        line_start: Some(10),
        line_end: Some(12),
        body: Some("fn answer() {}".to_string()),
    }];

    let response = build_evidence_only_response(
        "Where is answer?",
        AnswerQuality::Balanced,
        &investigate_response,
        &evidence,
        evidence.len(),
    );

    assert_eq!(response["response_shape"], "compact_evidence");
    assert_eq!(response["pack"]["coverage"]["status"], "complete");
    assert_eq!(response["pack"]["rows"][0]["file_path"], "src/lib.rs");
    assert_eq!(response["evidence"][0]["file_path"], "src/lib.rs");
    assert_eq!(response["evidence"][0]["line_start"], 10);
    assert_eq!(response["evidence"][0]["body"], "fn answer() {}");
    assert!(response["follow_up"]
        .as_str()
        .unwrap()
        .contains("Do not call Grep, Read, or Glob"));
}
```

- [ ] **Step 2: Run the focused test and verify failure**

Run:

```bash
cargo test -q handlers::ask_code::tests::evidence_only_response_marks_compact_and_preserves_grounding_fields
```

Expected: fail because `response_shape` and the stronger follow-up guidance do not exist yet.

- [ ] **Step 3: Update `build_evidence_only_response`**

In `src/handlers/ask_code.rs`, add `"response_shape": "compact_evidence"` to the returned JSON and update the non-empty-evidence `follow_up` string so it says:

```text
ask_code returned verified compact evidence without LLM prose (Path 2 default). Synthesise the final answer yourself from the `evidence[]` array below: each item carries symbol_name, file_path, line range, and the actual code body. Use these as the source of truth -- they were already retrieved and shape-classified by `investigate`. If `pack.coverage.status` is complete and the needed line-level source is present, Do not call Grep, Read, or Glob to re-check the same files. Call specialist tools only for partial/no_hits coverage, candidate rows, or missing source bodies needed for citation.
```

- [ ] **Step 4: Add the same marker to synthesized and unavailable responses**

Add `"response_shape": "compact_evidence"` to `build_unavailable_response` and `build_synthesized_response` so cached or alternate response paths expose a consistent field.

- [ ] **Step 5: Run focused `ask_code` tests**

Run:

```bash
cargo test -q handlers::ask_code::tests
```

Expected: all ask_code tests pass.

## Task 4: Tantivy Natural-Language Query Tolerance

- [ ] **Step 1: Write the failing Tantivy punctuation test**

Add this test to `src/storage/tantivy.rs` near the existing persisted search tests:

```rust
#[test]
fn natural_language_query_with_punctuation_does_not_fail_parse() {
    let dir = tmp_index_dir();
    let index = TantivyIndex::open_or_create(&dir).unwrap();
    index
        .upsert_symbol(
            &sample_symbol(
                "id1",
                "emitToolUse",
                "tool-use event flows from Claude provider session to renderer",
            ),
            "",
            "",
            None,
        )
        .unwrap();
    index.commit().unwrap();

    let hits = index
        .search(
            "Trace tool-use event flows Claude provider session out renderer. Name hop: event produced provider, session manager bridges IPC, IPC channel constant, renderer subscribes.",
            10,
        )
        .unwrap();

    assert!(hits.iter().any(|h| h.id == "id1"));
}
```

- [ ] **Step 2: Run the focused test and verify failure**

Run:

```bash
cargo test -q storage::tantivy::tests::natural_language_query_with_punctuation_does_not_fail_parse
```

Expected: fail with `Failed to parse tantivy query`.

- [ ] **Step 3: Use Tantivy lenient query parsing**

In `src/storage/tantivy.rs`, change `search_in_fields` to use `parse_query_lenient(query).0` instead of strict `parse_query(query)`. Keep the same `TopDocs` search flow and field boosts.

- [ ] **Step 4: Run focused Tantivy tests**

Run:

```bash
cargo test -q storage::tantivy::tests::natural_language_query_with_punctuation_does_not_fail_parse
cargo test -q storage::tantivy::tests::indexes_and_searches_persisted_docs
```

Expected: both tests pass.

## Task 5: Full Verification and Benchmark Smoke

- [ ] **Step 1: Run Python harness tests**

Run:

```bash
python3 -m unittest scripts.agent_qa.tests.test_bench_agent_qa
```

Expected: all tests pass.

- [ ] **Step 2: Run Rust tests for touched modules**

Run:

```bash
cargo test -q tools::tests
cargo test -q handlers::ask_code::tests
cargo test -q storage::tantivy::tests::natural_language_query_with_punctuation_does_not_fail_parse
```

Expected: all tests pass.

- [ ] **Step 3: Build release binary**

Run:

```bash
cargo build --release
```

Expected: build completes successfully.

- [ ] **Step 4: Run targeted Pylon smoke benchmark**

Run:

```bash
BENCH_TOOLSETS=default,code_intel python3 scripts/bench_agent_qa.py --round 5 --repo custom --base-dir /Users/dikrana/Documents/workspace/pylon --queries scripts/queries_qa_pylon.json --output-dir docs/benchmark_rounds/agent_pylon_token_reduction --question-ids pylon-q1,pylon-q9 --agent-timeout 600 --skip-judge
```

Expected: benchmark completes. Compare `code_intel` fallback `Read`/`Grep` calls and input tokens against R004 for `pylon-q1` and `pylon-q9`.

- [ ] **Step 5: Commit implementation**

Stage only touched source, tests, plan, and benchmark smoke artifacts if they are useful evidence:

```bash
git add scripts/bench_agent_qa.py scripts/agent_qa/tests/test_bench_agent_qa.py src/tools/mod.rs src/handlers/ask_code.rs src/storage/tantivy.rs docs/superpowers/plans/2026-05-25-token-reduction.md
git commit -m "perf: reduce code-intel token usage"
```

If smoke benchmark artifacts are produced and should be retained, stage them in a separate evidence commit:

```bash
git add docs/benchmark_rounds/agent_pylon_token_reduction
git commit -m "test: add token reduction benchmark smoke"
```
