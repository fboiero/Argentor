#!/usr/bin/env bash
# Publish the argentor-sdk TypeScript package to npm.
#
# Use when the CI publish-typescript job fails (typically because the
# NPM_TOKEN in GitHub Secrets is a granular-access token without
# permission to create the package). This publishes from your local
# logged-in npm session instead.
#
# Run from anywhere in the repo:
#
#   ./scripts/publish-sdk-npm.sh
#
# The script:
#   1. Builds and packs the SDK if no current tarball exists.
#   2. Checks you have an active npm login. If not, runs `npm login`
#      (interactive — you'll be prompted for username, password and OTP
#      or sent to a browser).
#   3. Publishes the tarball with public access.
#   4. Cleans up the tarball so it doesn't drift in the working tree.
#
# Exit codes:
#   0  publish succeeded
#   1  build, pack, or publish failed
#   2  user is not logged in and `npm login` failed
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SDK_DIR="$REPO_ROOT/sdks/typescript"
cd "$SDK_DIR"

PKG_NAME=$(node -p "require('./package.json').name")
PKG_VERSION=$(node -p "require('./package.json').version")
TARBALL="${PKG_NAME}-${PKG_VERSION}.tgz"

echo "→ Package: $PKG_NAME@$PKG_VERSION"
echo "→ SDK dir: $SDK_DIR"

# 1. Build + pack if no current tarball.
if [[ ! -f "$TARBALL" ]]; then
  echo "→ No tarball found, building and packing..."
  npm install --silent
  npm run build
  npm pack
else
  echo "→ Reusing existing tarball: $TARBALL"
fi

# 2. Verify version not already on npm.
if npm view "$PKG_NAME@$PKG_VERSION" version >/dev/null 2>&1; then
  echo "✓ $PKG_NAME@$PKG_VERSION is already published. Nothing to do."
  rm -f "$TARBALL"
  exit 0
fi

# 3. Login check. `npm whoami` exits non-zero with ENEEDAUTH when no session.
if ! npm whoami >/dev/null 2>&1; then
  echo "→ Not logged in to npm. Running 'npm login'..."
  echo "   (You'll be prompted for credentials. Use a browser-based login if 2FA is enabled.)"
  if ! npm login; then
    echo "✗ npm login failed. Try 'npm login --auth-type=web' for a browser flow." >&2
    exit 2
  fi
fi

NPM_USER=$(npm whoami)
echo "→ Logged in as: $NPM_USER"

# 4. Publish the tarball.
echo "→ Publishing $TARBALL..."
if npm publish --access public "$TARBALL"; then
  echo "✓ Published $PKG_NAME@$PKG_VERSION"
  echo "   https://www.npmjs.com/package/$PKG_NAME/v/$PKG_VERSION"
  rm -f "$TARBALL"
  exit 0
else
  echo "✗ Publish failed. The tarball is preserved at $SDK_DIR/$TARBALL" >&2
  exit 1
fi
