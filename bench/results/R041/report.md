# Bench Round R041

**Repos:** django, workings  **Arms:** 1  **Daemon SHA:** 49cab3092fda6de5fabdc4046438208ef83c7dbf  **Codegraph:** not installed  **Agent:** claude-sonnet-4-6

## Headline

(insufficient data for headline)

## Per-arm aggregate

| arm | n | judge | mech | citation | tools | tokens | tok/judge-pt | capped | wall |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| code_intel_shipped | 80 | 8.55 ±0.62 | 0.93 | 0.93 | 6.2 | 134,115 | 15,686 | 0% | 47s |

## Reproducibility

**Comparator:** code_intel_shipped → none  **Daemon binary SHA-256:** 0ecc7d65b1ca696c832c29e866b71d514f230b2f5c1487f1213d3a91eb8cacec  **Agent CLI:** 2.1.205 (Claude Code)

### Fixture revisions

| repo | upstream SHA | fixture SHA-256 | schema | questions |
|---|---|---|---:|---:|
| django | 2d4add11fd57b05f7ea48e8b3e89e743c9871aa3 | 39f4bed2e14333e4f21ad63a322cf41e035577cb0d3fb4b727437716b1e20863 | 22 | 20 |
| workings | 53f9d71cb800ff9df2bb3636e9f803f3a16bf249 | 4645e9686f9bde2acbaaee9bb71c5202d4c094ed462d49fb3197a7d4e0331ca4 | 22 | 20 |

### Models and execution

| role | model |
|---|---|
| agent | claude-sonnet-4-6 |
| judge/haiku | claude-haiku-4-5 |
| judge/opus | claude-opus-4-8 |
| judge/sonnet | claude-sonnet-4-6 |

### Arm configuration

| arm | index | daemon env |
|---|---|---|
| code_intel_shipped | no_desc | BENCH_DISABLE_DESCRIPTIONS=1 |

## Failures worth inspecting

### high_judge_disagreement
- (none)

### hallucinated_citations
- (none)

### forbidden_hits
- (none)

### regressed_vs_full
- (none)
