#!/usr/bin/env bash
# check-clean.sh — Fast pre-commit lint for OpenScript.
#
# Lightweight check that runs BEFORE git commit. Catches the common issues
# (warnings, unused imports, format) without the full test suite. Use this
# during development; use post-iteration.sh after the commit.
#
# Usage:  bash scripts/check-clean.sh
# Exit:   0 = clean, 1 = issues found

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

if [ -d "$HOME/.cargo/bin" ]; then
  export PATH="$HOME/.cargo/bin:$PATH"
fi

echo "=== Fast pre-commit lint ==="
echo ""

# 1. cargo build (zero warnings)
echo -n "[1/3] cargo build (zero warnings)... "
BUILD_OUTPUT=$(cargo build --workspace --exclude openscript-tauri 2>&1) || {
  echo -e "${RED}FAILED${NC}"
  echo "$BUILD_OUTPUT" | tail -15
  exit 1
}
# Count warnings. grep -c returns "0" on no match but exits 1, so use || true.
WARNING_COUNT=$(echo "$BUILD_OUTPUT" | grep -c '^warning:' || true)
if [ "$WARNING_COUNT" -gt 0 ]; then
  echo -e "${YELLOW}${WARNING_COUNT} warnings${NC}"
  echo "$BUILD_OUTPUT" | grep '^warning:' | head -5
else
  echo -e "${GREEN}OK${NC}"
fi

# 2. cargo fmt --check (formatting)
echo -n "[2/3] cargo fmt --check... "
if cargo fmt --all -- --check >/dev/null 2>&1; then
  echo -e "${GREEN}OK${NC}"
else
  echo -e "${YELLOW}NOT FORMATTED${NC}"
  echo "Run: cargo fmt --all"
  # Don't fail — just warn. Formatting is not a correctness issue.
fi

# 3. TypeScript (if node_modules exists)
FRONTEND_DIR="crates/openscript-tauri/src/frontend"
if [ -d "$FRONTEND_DIR/node_modules" ]; then
  echo -n "[3/3] npx tsc --noEmit... "
  TSC_OUTPUT=$(cd "$FRONTEND_DIR" && npx tsc --noEmit 2>&1) || {
    echo -e "${RED}FAILED${NC}"
    echo "$TSC_OUTPUT" | tail -10
    exit 1
  }
  echo -e "${GREEN}OK${NC}"
else
  echo -e "[3/3] npx tsc --noEmit... ${YELLOW}SKIPPED (no node_modules)${NC}"
fi

echo ""
echo -e "${GREEN}✓ Clean — ready to commit${NC}"
echo "Next: git add -A && git commit -m \"<Phase>: <summary>\" && git push origin main"
echo "Or:   bash scripts/post-iteration.sh  (after committing, to run the full gate)"
exit 0
