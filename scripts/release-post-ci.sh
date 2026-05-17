#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: ./scripts/release-post-ci.sh [vX.Y.Z] [options]

Run after `./scripts/release.sh` has tagged and pushed a release.
This script:

  1. Waits for the matching "Release" workflow run to finish on GitHub.
  2. Downloads the macOS tarball from the GitHub release and computes
     its sha256.
  3. Rewrites the `REPLACE_ON_RELEASE` sentinel in
     `dist/homebrew/code-intelligence-mcp.rb`, commits, and pushes.
  4. Copies the formula into the sibling `../homebrew-tap` checkout
     (override with --tap-dir or HOMEBREW_TAP_DIR), commits, and pushes.
  5. If RELEASE_NOTES.md exists, attaches it to the GitHub release via
     `gh release edit`.

Arguments:
  vX.Y.Z            Release tag. Defaults to the latest tag in this repo.

Options:
  --tap-dir PATH    Path to the homebrew-tap checkout.
                    Default: ../homebrew-tap (or $HOMEBREW_TAP_DIR).
  --notes-file PATH Release notes file to attach. Default: RELEASE_NOTES.md.
  --no-wait         Skip `gh run watch`; assume the release is already built.
  --no-push         Commit locally but don't push (also skips tap-repo push).
  --skip-notes      Don't attach release notes even if the file exists.
  --dry-run         Print every action; modify nothing.
  -h, --help        Show this help.

Requirements:
  - `gh` CLI authenticated for iceinvein/code_intelligence_mcp_server and
    iceinvein/homebrew-tap.
  - Clean working trees in both repos.
EOF
}

SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
REPO_ROOT="$(dirname "$SCRIPT_DIR")"
cd "$REPO_ROOT"

# --- Parse args -----------------------------------------------------------

TAG=""
TAP_DIR="${HOMEBREW_TAP_DIR:-../homebrew-tap}"
NOTES_FILE="RELEASE_NOTES.md"
WAIT=1
PUSH=1
SKIP_NOTES=0
DRY_RUN=0

while [ $# -gt 0 ]; do
  case "$1" in
    -h|--help)     usage; exit 0 ;;
    --tap-dir)     shift; TAP_DIR=$1 ;;
    --notes-file)  shift; NOTES_FILE=$1 ;;
    --no-wait)     WAIT=0 ;;
    --no-push)     PUSH=0 ;;
    --skip-notes)  SKIP_NOTES=1 ;;
    --dry-run)     DRY_RUN=1 ;;
    v[0-9]*)
      if [ -n "$TAG" ]; then
        echo "Multiple tags supplied: '$TAG' and '$1'" >&2
        exit 1
      fi
      TAG=$1
      ;;
    *) echo "Unknown argument: $1" >&2; usage; exit 1 ;;
  esac
  shift
done

if [ -z "$TAG" ]; then
  TAG=$(git describe --tags --abbrev=0 2>/dev/null || true)
  if [ -z "$TAG" ]; then
    echo "No git tag found. Pass an explicit vX.Y.Z." >&2
    exit 1
  fi
fi

if ! [[ "$TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+([.+-].*)?$ ]]; then
  echo "Invalid tag format: '$TAG' (expected vX.Y.Z)" >&2
  exit 1
fi

VERSION="${TAG#v}"
TARBALL="code-intelligence-mcp-server-aarch64-apple-darwin.tar.gz"
FORMULA="dist/homebrew/code-intelligence-mcp.rb"
TAP_FORMULA_REL="Formula/code-intelligence-mcp.rb"

echo "Tag:             $TAG"
echo "Version:         $VERSION"
echo "Tap directory:   $TAP_DIR"
echo "Notes file:      $NOTES_FILE"
[ "$DRY_RUN" -eq 1 ] && echo "Mode:            DRY-RUN"

# --- Dependencies ---------------------------------------------------------

for tool in gh git shasum sed awk; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "Required tool '$tool' not found on PATH." >&2
    exit 1
  fi
done

if ! gh auth status >/dev/null 2>&1; then
  echo "gh CLI is not authenticated. Run 'gh auth login' first." >&2
  exit 1
fi

# --- Cleanliness ----------------------------------------------------------

require_clean() {
  local dir=$1
  if ! git -C "$dir" diff --quiet || ! git -C "$dir" diff --cached --quiet; then
    echo "Working tree in '$dir' is dirty. Commit or stash first." >&2
    exit 1
  fi
}

require_clean "$REPO_ROOT"

if [ ! -d "$TAP_DIR/.git" ]; then
  echo "Homebrew tap not found at '$TAP_DIR' (expected a git checkout)." >&2
  echo "Clone it with: gh repo clone iceinvein/homebrew-tap $TAP_DIR" >&2
  exit 1
fi
require_clean "$TAP_DIR"

# --- Wait for the release workflow ---------------------------------------

run() {
  if [ "$DRY_RUN" -eq 1 ]; then
    printf '[dry-run] %s\n' "$*"
  else
    eval "$@"
  fi
}

if [ "$WAIT" -eq 1 ]; then
  echo "Looking for Release workflow run for $TAG ..."
  # gh stores the tag in headBranch for tag-triggered runs.
  RUN_ID=""
  for _ in 1 2 3 4 5; do
    RUN_ID=$(gh run list \
      --workflow release.yml \
      --json databaseId,headBranch,status,conclusion \
      --limit 30 \
      --jq ".[] | select(.headBranch == \"$TAG\") | .databaseId" \
      | head -1)
    if [ -n "$RUN_ID" ]; then break; fi
    echo "  no run yet, retrying in 10s ..."
    sleep 10
  done

  if [ -z "$RUN_ID" ]; then
    echo "Could not find a Release workflow run for $TAG." >&2
    echo "Re-run with --no-wait once it appears, or check 'gh run list'." >&2
    exit 1
  fi

  echo "Watching run $RUN_ID ..."
  if [ "$DRY_RUN" -eq 0 ]; then
    gh run watch "$RUN_ID" --exit-status
  else
    echo "[dry-run] gh run watch $RUN_ID --exit-status"
  fi
fi

# --- Download tarball and compute sha256 ---------------------------------

TMPDIR_LOCAL=$(mktemp -d)
trap 'rm -rf "$TMPDIR_LOCAL"' EXIT

echo "Downloading $TARBALL from release $TAG ..."
if [ "$DRY_RUN" -eq 0 ]; then
  gh release download "$TAG" -p "$TARBALL" -D "$TMPDIR_LOCAL" --clobber
else
  echo "[dry-run] gh release download $TAG -p $TARBALL -D $TMPDIR_LOCAL"
fi

if [ "$DRY_RUN" -eq 0 ]; then
  SHA=$(shasum -a 256 "$TMPDIR_LOCAL/$TARBALL" | awk '{print $1}')
else
  SHA="REPLACE_ON_RELEASE"  # placeholder so the rest of the dry-run flows
fi
echo "sha256: $SHA"

# --- Rewrite the formula and commit --------------------------------------

if [ ! -f "$FORMULA" ]; then
  echo "Formula not found: $FORMULA" >&2
  exit 1
fi

CURRENT_SHA=$(sed -nE 's/^[[:space:]]*sha256 "(.+)"$/\1/p' "$FORMULA" | head -1)
if [ "$CURRENT_SHA" = "$SHA" ]; then
  echo "Formula already pinned to $SHA. Skipping main-repo commit."
  MAIN_COMMITTED=0
else
  echo "Patching $FORMULA: $CURRENT_SHA -> $SHA"
  if [ "$DRY_RUN" -eq 0 ]; then
    sed -i '' "s|^      sha256 \".*\"$|      sha256 \"$SHA\"|" "$FORMULA"
    if ! grep -q "sha256 \"$SHA\"" "$FORMULA"; then
      echo "Failed to write sha256 into $FORMULA" >&2
      exit 1
    fi
    git add "$FORMULA"
    git commit -m "chore(brew): pin $TAG sha256"
  else
    echo "[dry-run] sed sha256, git add $FORMULA, git commit -m 'chore(brew): pin $TAG sha256'"
  fi
  MAIN_COMMITTED=1
fi

if [ "$MAIN_COMMITTED" -eq 1 ] && [ "$PUSH" -eq 1 ]; then
  run "git push origin HEAD"
fi

# --- Sync to the tap repo ------------------------------------------------

TAP_FORMULA="$TAP_DIR/$TAP_FORMULA_REL"
echo "Syncing formula to $TAP_FORMULA ..."

if [ "$DRY_RUN" -eq 0 ]; then
  mkdir -p "$(dirname "$TAP_FORMULA")"
  cp "$FORMULA" "$TAP_FORMULA"
else
  echo "[dry-run] cp $FORMULA $TAP_FORMULA"
fi

if git -C "$TAP_DIR" diff --quiet -- "$TAP_FORMULA_REL" \
   && git -C "$TAP_DIR" diff --cached --quiet -- "$TAP_FORMULA_REL"; then
  echo "Tap formula already up to date. Skipping tap commit."
else
  if [ "$DRY_RUN" -eq 0 ]; then
    git -C "$TAP_DIR" add "$TAP_FORMULA_REL"
    git -C "$TAP_DIR" commit -m "code-intelligence-mcp $TAG"
    if [ "$PUSH" -eq 1 ]; then
      git -C "$TAP_DIR" push origin HEAD
    fi
  else
    echo "[dry-run] git -C $TAP_DIR add $TAP_FORMULA_REL"
    echo "[dry-run] git -C $TAP_DIR commit -m 'code-intelligence-mcp $TAG'"
    [ "$PUSH" -eq 1 ] && echo "[dry-run] git -C $TAP_DIR push origin HEAD"
  fi
fi

# --- Attach release notes ------------------------------------------------

if [ "$SKIP_NOTES" -eq 0 ] && [ -s "$NOTES_FILE" ]; then
  echo "Attaching $NOTES_FILE to release $TAG ..."
  if [ "$DRY_RUN" -eq 0 ]; then
    gh release edit "$TAG" --notes-file "$NOTES_FILE"
  else
    echo "[dry-run] gh release edit $TAG --notes-file $NOTES_FILE"
  fi
elif [ "$SKIP_NOTES" -eq 1 ]; then
  echo "Skipping release-notes attach (--skip-notes)."
else
  echo "No $NOTES_FILE found; skipping release-notes attach."
fi

echo ""
echo "--------------------------------------------------"
echo "Post-CI release tasks complete for $TAG."
echo "--------------------------------------------------"
if [ "$PUSH" -eq 0 ]; then
  echo "Note: --no-push was set. Push manually when ready:"
  echo "  git -C $REPO_ROOT push origin HEAD"
  echo "  git -C $TAP_DIR push origin HEAD"
fi
