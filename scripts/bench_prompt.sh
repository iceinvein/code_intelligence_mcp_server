#!/usr/bin/env bash
# Generate the benchmark prompt for the next round.
# Reads the last round number and averages from SEARCH_BENCHMARK.md,
# increments the round, and outputs a ready-to-paste prompt.
#
# Usage:
#   ./scripts/bench_prompt.sh          # print to stdout
#   ./scripts/bench_prompt.sh | pbcopy # copy to clipboard (macOS)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BENCH_FILE="$SCRIPT_DIR/../docs/SEARCH_BENCHMARK.md"

if [[ ! -f "$BENCH_FILE" ]]; then
  echo "Error: $BENCH_FILE not found" >&2
  exit 1
fi

# Find the last "### Round N" heading (portable: no grep -P)
LAST_ROUND=$(grep '### Round [0-9]' "$BENCH_FILE" | sed 's/.*### Round \([0-9]*\).*/\1/' | tail -1)
if [[ -z "$LAST_ROUND" ]]; then
  echo "Error: No round found in $BENCH_FILE" >&2
  exit 1
fi

NEXT_ROUND=$((LAST_ROUND + 1))

# Extract the first few lines of the last round section
ROUND_SECTION=$(sed -n "/### Round $LAST_ROUND /,/### Round /p" "$BENCH_FILE" | head -5)

# Extract CI and Augment averages (portable sed)
CI_AVG=$(echo "$ROUND_SECTION" | sed -n 's/.*CI average: \*\*\([0-9.]*\).*/\1/p' | head -1)
AUG_AVG=$(echo "$ROUND_SECTION" | sed -n 's/.*Augment average: \*\*\([0-9.]*\).*/\1/p' | head -1)

if [[ -z "$CI_AVG" || -z "$AUG_AVG" ]]; then
  echo "Warning: Could not extract averages from Round $LAST_ROUND, using placeholders" >&2
  CI_AVG="X.X"
  AUG_AVG="X.X"
fi

cat <<PROMPT
Run a full 15-query search quality benchmark round using the autonomous
agent workflow described in docs/SEARCH_BENCHMARK.md under "How to Run
(Autonomous Agent Workflow)".

The agent prompt template is at docs/benchmark_rounds/AGENT_PROMPT_TEMPLATE.md.
This is Round ${NEXT_ROUND}. Dispatch all 3 batches in parallel with
run_in_background: true, writing to:
- docs/benchmark_rounds/round_${NEXT_ROUND}_batch_1.md
- docs/benchmark_rounds/round_${NEXT_ROUND}_batch_2.md
- docs/benchmark_rounds/round_${NEXT_ROUND}_batch_3.md

After all 3 complete, read the batch files, compile the full round table,
and compare deltas to Round ${LAST_ROUND} (CI avg ${CI_AVG}, Augment avg ${AUG_AVG}).

Then analyze the results:
1. Flag regressions (>1 point drop from previous round)
2. Group queries scoring CI < 6 by failure pattern
3. For each pattern affecting 2+ queries, propose a fix:
   - File to modify and what to change
   - Which queries should improve
   - Regression risk
4. Append the compiled round AND analysis to docs/SEARCH_BENCHMARK.md.
PROMPT
