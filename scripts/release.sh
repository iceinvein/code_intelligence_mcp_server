#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: ./scripts/release.sh <patch|minor|major|x.y.z> [options]

Bump the project version (Cargo + npm packages), generate release notes via
the local `claude` CLI, then commit and tag.

Bump types:
  patch    Increment patch  (3.0.0 -> 3.0.1)
  minor    Increment minor  (3.0.0 -> 3.1.0)
  major    Increment major  (3.0.0 -> 4.0.0)

You may also pass a literal version like 3.1.4.

Options:
  --no-notes        Skip release-notes generation
  --notes-only      Generate notes only; do not bump version, commit, or tag
  --dry-run         Print what would happen; do not modify anything
  --notes-file PATH Path for the generated notes file (default: RELEASE_NOTES.md)
  --since TAG       Override the "since" tag for the changelog (default: latest)
  -h, --help        Show this help
EOF
}

if [ $# -lt 1 ]; then
  usage
  exit 1
fi

ARG=$1
shift

case "$ARG" in
  -h|--help) usage; exit 0 ;;
esac

NO_NOTES=0
NOTES_ONLY=0
DRY_RUN=0
NOTES_FILE="RELEASE_NOTES.md"
SINCE_TAG=""

while [ $# -gt 0 ]; do
  case "$1" in
    --no-notes)    NO_NOTES=1 ;;
    --notes-only)  NOTES_ONLY=1 ;;
    --dry-run)     DRY_RUN=1 ;;
    --notes-file)  shift; NOTES_FILE=$1 ;;
    --since)       shift; SINCE_TAG=$1 ;;
    -h|--help)     usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage; exit 1 ;;
  esac
  shift
done

SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
REPO_ROOT="$(dirname "$SCRIPT_DIR")"
cd "$REPO_ROOT"

# --- Resolve current and next version --------------------------------------

CURRENT_VERSION=$(sed -n '3s/^version = "\(.*\)"/\1/p' Cargo.toml)
if [ -z "$CURRENT_VERSION" ]; then
  echo "Could not read current version from Cargo.toml (line 3)." >&2
  exit 1
fi

bump_version() {
  local current=$1 kind=$2 maj min pat
  IFS='.' read -r maj min pat <<<"$current"
  if ! [[ "$maj" =~ ^[0-9]+$ && "$min" =~ ^[0-9]+$ && "$pat" =~ ^[0-9]+$ ]]; then
    echo "Current version '$current' is not a valid semver triple." >&2
    exit 1
  fi
  case "$kind" in
    patch) pat=$((pat + 1)) ;;
    minor) min=$((min + 1)); pat=0 ;;
    major) maj=$((maj + 1)); min=0; pat=0 ;;
  esac
  echo "$maj.$min.$pat"
}

case "$ARG" in
  patch|minor|major)
    VERSION=$(bump_version "$CURRENT_VERSION" "$ARG")
    ;;
  *)
    if ! [[ "$ARG" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.+-].*)?$ ]]; then
      echo "Invalid bump type or version: '$ARG'" >&2
      usage
      exit 1
    fi
    VERSION=$ARG
    ;;
esac

if [ "$NOTES_ONLY" -eq 0 ]; then
  echo "Current version: $CURRENT_VERSION"
  echo "New version:     $VERSION"
fi

# --- Cleanliness check -----------------------------------------------------

if [ "$NOTES_ONLY" -eq 0 ] && [ "$DRY_RUN" -eq 0 ]; then
  if ! git diff --quiet || ! git diff --cached --quiet; then
    echo "Working tree is dirty. Commit or stash changes first." >&2
    exit 1
  fi
fi

# --- Release notes ---------------------------------------------------------

generate_release_notes() {
  local version=$1 notes_file=$2 since_tag=$3
  local last_tag range commits diffstat prompt

  if [ -n "$since_tag" ]; then
    last_tag=$since_tag
  else
    last_tag=$(git describe --tags --abbrev=0 2>/dev/null || true)
  fi

  if [ -n "$last_tag" ]; then
    range="$last_tag..HEAD"
    echo "Generating release notes for commits in $range ..."
  else
    range="HEAD"
    echo "Generating release notes for all commits ..."
  fi

  commits=$(git log "$range" --pretty=format:'- %h %s (%an)' --no-merges)
  if [ -z "$commits" ]; then
    echo "No commits found in range $range. Skipping release notes." >&2
    return 1
  fi

  diffstat=$(git diff "$range" --stat 2>/dev/null | tail -n 60 || true)

  prompt=$(cat <<PROMPT
You are drafting GitHub release notes for code-intelligence-mcp-server v${version}.

Previous tag: ${last_tag:-(none)}
New version:  v${version}

Commits in this release:
${commits}

File change summary:
${diffstat}

Write concise, user-facing release notes in Markdown. Group changes under
"### Features", "### Fixes", "### Performance", "### Internal", or
"### Breaking changes" as appropriate (omit empty sections). Lead with a
short 1-2 sentence summary. Reference commit hashes inline like (\`abc1234\`).
Do not include emdashes (use periods, colons, or parentheses instead).
Do not include AI attribution. Do not include a top-level heading; the body
will be rendered under v${version} on GitHub.
PROMPT
)

  if [ "$DRY_RUN" -eq 1 ]; then
    echo "[dry-run] Would invoke: claude -p (prompt below)" >&2
    echo "----- prompt -----" >&2
    printf '%s\n' "$prompt" >&2
    echo "------------------" >&2
    return 0
  fi

  # shellcheck disable=SC2086
  printf '%s' "$prompt" | claude -p ${CLAUDE_FLAGS:-} > "$notes_file"

  if [ ! -s "$notes_file" ]; then
    echo "claude returned empty output; no notes written." >&2
    rm -f "$notes_file"
    return 1
  fi

  echo "Release notes written to $notes_file"
  echo "----- preview -----"
  sed -n '1,40p' "$notes_file"
  [ "$(wc -l <"$notes_file")" -gt 40 ] && echo "... (truncated)"
  echo "-------------------"

  if [ -t 0 ]; then
    read -r -p "Edit release notes? [y/N] " ans
    if [[ "$ans" =~ ^[Yy]$ ]]; then
      "${EDITOR:-vi}" "$notes_file"
    fi
  fi
  return 0
}

if [ "$NO_NOTES" -eq 0 ]; then
  if ! command -v claude >/dev/null 2>&1; then
    echo "Warning: 'claude' CLI not found on PATH; skipping release notes." >&2
    NO_NOTES=1
  fi
fi

if [ "$NO_NOTES" -eq 0 ]; then
  generate_release_notes "$VERSION" "$NOTES_FILE" "$SINCE_TAG" || NO_NOTES=1
fi

if [ "$NOTES_ONLY" -eq 1 ]; then
  echo "Notes-only run complete."
  exit 0
fi

# --- Apply version bump ----------------------------------------------------

if [ "$DRY_RUN" -eq 1 ]; then
  echo "[dry-run] Would update Cargo.toml, Cargo.lock, npm/package.json, npm-standalone/package.json"
  echo "[dry-run] Would commit and tag v$VERSION"
  exit 0
fi

# 1. Cargo.toml (only the package version on line 3)
sed -i '' "3s/^version = \".*\"/version = \"$VERSION\"/" Cargo.toml
echo "Updated Cargo.toml"

cargo check >/dev/null 2>&1 || true
echo "Updated Cargo.lock"

# 2/3. npm packages
cp README.md npm/README.md
cp README.md npm-standalone/README.md
( cd npm && npm pkg set version="$VERSION" >/dev/null )
( cd npm-standalone && npm pkg set version="$VERSION" >/dev/null )
echo "Updated npm package versions"

# 4. Commit and tag
git add Cargo.toml Cargo.lock \
  npm/package.json npm/README.md \
  npm-standalone/package.json npm-standalone/README.md

git commit -m "chore: release v$VERSION"
git tag "v$VERSION"

echo ""
echo "--------------------------------------------------"
echo "Release v$VERSION ready."
echo "--------------------------------------------------"
echo "Next steps:"
echo "  1. git show HEAD"
echo "  2. git push origin main && git push origin v$VERSION"
echo "  3. Wait for CI to publish the GitHub release."
if [ "$NO_NOTES" -eq 0 ]; then
  echo "  4. Attach the generated notes:"
  echo "       gh release edit v$VERSION --notes-file $NOTES_FILE"
fi
