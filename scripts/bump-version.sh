#!/bin/bash
set -e

# Version bumping script for Pandemic
NEW_VERSION="$1"

if [ -z "$NEW_VERSION" ]; then
    echo "Usage: $0 <new-version>"
    echo "Example: $0 0.2.0"
    exit 1
fi

echo "Bumping version to $NEW_VERSION..."

# Update Cargo.toml files
find . -name "Cargo.toml" -exec sed -i "s/^version = \".*\"/version = \"$NEW_VERSION\"/" {} \;

# Update any hardcoded versions in documentation
sed -i "s/pandemic v[0-9]\+\.[0-9]\+\.[0-9]\+/pandemic v$NEW_VERSION/g" README.md docs/index.md 2>/dev/null || true

echo "✅ Version bumped to $NEW_VERSION"
echo "Next steps:"
echo "1. git add ."
echo "2. git commit -m \"Bump version to v$NEW_VERSION\""
echo "3. git tag v$NEW_VERSION"
echo "4. git push origin main --tags"