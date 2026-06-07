# Fixture Authoring Guide

This document describes how to write good benchmark questions.

## Schema

See `bench/fixtures/smoke.yaml` for the canonical example.

Per question:

- `id`: stable identifier. Format: `<repo>-<task_type>-NN`.
- `task_type`: one of `symbol_lookup`, `concept`, `multi_hop`, `impact`, `architectural`, `negative`.
- `difficulty`: `easy` | `medium` | `hard` (metadata only, does not affect scoring).
- `question`: the question as posed to the agent.
- `rubric`: free-text describing what a correct answer looks like and what triggers penalties.
- `expected.citations[]`: canonical `file:line_range` answers. The agent's mech score depends on hitting at least one.
- `expected.files[]`: looser allow-list of files acceptable to mention.
- `expected.facts[]`: each entry is a string (one required term) or a list (OR alternatives, any one satisfies).
- `expected.forbidden[]`: substrings whose presence subtracts from mech.
- `expected.forbidden_strict`: when `true`, any forbidden hit zeroes the mech score.

## Authoring Rules

1. **Read the code first.** Capture `file:line_range` at authoring time, against the pinned `upstream_sha`. Do not rely on memory.
2. **Every question has at least one citation.** The bench must be falsifiable. Exception: negative-task questions where you are asking the agent to confirm something does NOT exist; in that case `citations` may be empty.
3. **Rubrics enumerate penalties explicitly.** Generic "be correct" rubrics let the judge be too generous. Name the plausible wrong answers.
4. **Forbidden lists name plausible-but-wrong targets.** While writing the question, ask "what is the sloppy near-miss?" If there is no near-miss, leave `forbidden` empty.
5. **Distribution per repo:** 4 `symbol_lookup`, 4 `concept`, 4 `multi_hop`, 3 `impact`, 3 `architectural`, 2 `negative`. 20 total.

## Validation

Run validation before committing fixture changes:

```bash
python3 -m bench.run validate bench/fixtures/<repo>.yaml --repo-root <path>
```

The validator checks:

- YAML parses cleanly against the schema.
- All question IDs are unique within the fixture.
- All `task_type` values are from the allowed set.
- All cited files exist under `repo-root` and line ranges are within bounds.
- `forbidden_strict: true` requires a non-empty `forbidden` list.

## Running the Bench

```bash
python3 -m bench.run --help              # list all subcommands
python3 -m bench.run validate <fixture>  # lint a fixture file
python3 -m bench.run list                # enumerate prior rounds in bench/results/
```

Full benchmark execution (prep, run, judge, report) is wired in Task 14 and later tasks.

## External Index Smoke Fixtures

Use `external-index-smoke.yaml` for fast regression checks of provenance overlay behavior. These cases verify that precise external references are preferred over Tree-sitter fallback rows and that callsite/impact answers preserve explicit provenance.
