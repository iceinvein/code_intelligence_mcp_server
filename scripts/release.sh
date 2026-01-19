#!/bin/bash
set -e

if [ -z "$1" ]; then
  echo "Usage: ./scripts/release.sh <version>"
  echo "Example: ./scripts/release.sh 0.1.0"
  exit 1
fi

VERSION=$1

# Ensure we are in the project root
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
REPO_ROOT="$(dirname "$SCRIPT_DIR")"
cd "$REPO_ROOT"

echo "Releasing version $VERSION..."

# 1. Update Cargo.toml (Rust)
# We use a slight trick to only match the first occurrence of version = "..." which is the package version
if [[ "$OSTYPE" == "darwin"* ]]; then
  # macOS sed requires empty string for -i extension
  sed -i '' "3s/^version = \".*\"/version = \"$VERSION\"/" Cargo.toml
else
  sed -i "3s/^version = \".*\"/version = \"$VERSION\"/" Cargo.toml
fi
echo "Updated Cargo.toml"

# Update Cargo.lock to reflect the new version
cargo check > /dev/null 2>&1 || true
echo "Updated Cargo.lock"

# 2. Update npm/package.json (Node)
# Copy README to npm package
cp README.md npm/README.md
echo "Copied README.md to npm/"

cd npm
npm pkg set version=$VERSION
cd ..
echo "Updated npm/package.json"

# 3. Commit and Tag
echo "Creating git commit and tag..."
git add Cargo.toml Cargo.lock npm/package.json npm/README.md
git commit -m "chore: release v$VERSION"
git tag "v$VERSION"

echo "--------------------------------------------------"
echo "Release v$VERSION ready!"
echo "--------------------------------------------------"
echo "Next steps:"
echo "1. Verify the changes: git show HEAD"
echo "2. Push to GitHub:     git push origin main && git push origin v$VERSION"
echo "3. Wait for CI build to complete."
echo "4. Publish NPM:        cd npm && npm publish --access public"
