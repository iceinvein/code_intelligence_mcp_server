# Bench Round R005

**Repos:** django, wolfmax  **Arms:** 5  **Daemon SHA:** ?  **Codegraph:** not installed  **Agent:** claude-sonnet-4-6

## Headline

code_intel_full vs default: +0.8 judge / +0.05 mech.
code_intel_no_descriptions vs code_intel_full: +0.0 judge.
code_intel_no_reranker vs code_intel_full: +0.2 judge.
codegraph vs default: +0.8 judge.

## Per-arm aggregate

| arm | n | judge | mech | citation | tools | tokens | wall |
|---|---:|---:|---:|---:|---:|---:|---:|
| default | 40 | 6.08 ±1.85 | 0.41 | 0.53 | 5.2 | 164,054 | 52s |
| code_intel_full | 40 | 6.92 ±2.45 | 0.46 | 0.60 | 5.0 | 175,816 | 40s |
| code_intel_no_descriptions | 40 | 6.92 ±2.25 | 0.43 | 0.50 | 5.1 | 176,820 | 37s |
| code_intel_no_reranker | 40 | 7.12 ±2.17 | 0.43 | 0.50 | 4.6 | 172,276 | 36s |
| codegraph | 40 | 6.92 ±2.15 | 0.33 | 0.33 | 7.0 | 218,739 | 47s |

## Failures worth inspecting

### high_judge_disagreement
- (none)

### hallucinated_citations
- (none)

### forbidden_hits
- (none)

### regressed_vs_full
- (none)
