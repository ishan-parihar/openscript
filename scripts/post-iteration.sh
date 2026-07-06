#!/usr/bin/env bash
# post-iteration.sh — Automated post-iteration gate for OpenScript.
#
# Enforces the Definition of Done from AGENTS.md §7:
#   1. cargo build — zero warnings
#   2. cargo test — all pass, count >= baseline (216)
#   3. npx tsc --noEmit — clean (if frontend exists)
#   4. git status — no uncommitted changes
#   5. git push origin main — succeeds
#   6. git log origin/main..HEAD — empty (nothing unpushed)
#
# Usage:  bash scripts/post-iteration.sh
# Exit:   0 = PASS (iteration is truly done), 1 = FAIL (fix it)
#
# This script is the automated enforcement of the iron rule. Run it after
# every iteration. If it fails, the iteration is NOT done — fix the failure
# before starting the next iteration.

set -euo pipefail

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Resolve the repo root (script is in scripts/)
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# Ensure cargo is on PATH (survives container resets where ~/.cargo may be gone)
if [ -d "$HOME/.cargo/bin" ]; then
  export PATH="$HOME/.cargo/bin:$PATH"
fi

# Ensure git credential helper is set (survives container resets)
if [ -f /home/z/my-project/.git-credentials ]; then
  git config --global credential.helper "store --file=/home/z/my-project/.git-credentials" 2>/dev/null || true
fi

# Test baseline — must never decrease. Update when adding new tests.
BASELINE_TEST_COUNT=233

FAIL() {
  echo ""
  echo -e "${RED}✗ POST-ITERATION GATE FAILED${NC}"
  echo -e "${RED}  → $1${NC}"
  echo ""
  echo -e "${YELLOW}The iteration is NOT done. Fix the failure above before starting the next iteration.${NC}"
  echo -e "${YELLOW}See AGENTS.md §7 for the protocol.${NC}"
  exit 1
}

PASS() {
  echo ""
  echo -e "${GREEN}✓ POST-ITERATION GATE PASSED${NC}"
  echo -e "${GREEN}  Build: clean (zero warnings)${NC}"
  echo -e "${GREEN}  Tests: ${test_count} pass (baseline ${BASELINE_TEST_COUNT})${NC}"
  echo -e "${GREEN}  TypeScript: clean${NC}"
  echo -e "${GREEN}  Git: working tree clean, all commits pushed to origin/main${NC}"
  echo ""
  echo -e "${GREEN}The iteration is done.${NC}"
  exit 0
}

echo "=== Post-Iteration Gate ==="
echo "Repo: $REPO_ROOT"
echo ""

# ---- Step 1: cargo build (zero warnings) ----
echo -n "[1/7] cargo build (zero warnings)... "
BUILD_OUTPUT=$(cargo build --workspace --exclude openscript-tauri 2>&1) || {
  echo -e "${RED}FAILED${NC}"
  echo "$BUILD_OUTPUT" | tail -20
  FAIL "cargo build failed"
}
# Check for warnings (the build succeeded, but warnings = fail)
WARNING_COUNT=$(echo "$BUILD_OUTPUT" | grep -c '^warning:' || true)
if [ "$WARNING_COUNT" -gt 0 ]; then
  echo -e "${RED}FAILED (${WARNING_COUNT} warnings)${NC}"
  echo "$BUILD_OUTPUT" | grep '^warning:' | head -10
  FAIL "cargo build has ${WARNING_COUNT} warnings — fix them"
fi
echo -e "${GREEN}OK${NC}"

# ---- Step 2: cargo test (all pass, count >= baseline) ----
echo -n "[2/7] cargo test (>= ${BASELINE_TEST_COUNT} tests)... "
TEST_OUTPUT=$(cargo test --workspace --exclude openscript-tauri --lib --bins --tests 2>&1) || {
  echo -e "${RED}FAILED${NC}"
  echo "$TEST_OUTPUT" | tail -30
  FAIL "cargo test failed (some tests failed)"
}
# Extract test count from "test result: ok. N passed; ..."
test_count=$(echo "$TEST_OUTPUT" | grep -E "^test result:" | awk '{n+=$4} END {print n+0}')
if [ -z "$test_count" ] || [ "$test_count" -lt "$BASELINE_TEST_COUNT" ]; then
  echo -e "${RED}FAILED${NC}"
  echo "Test count: ${test_count:-0} (baseline: ${BASELINE_TEST_COUNT})"
  FAIL "test count ${test_count:-0} < baseline ${BASELINE_TEST_COUNT} — did tests get deleted?"
fi
echo -e "${GREEN}OK (${test_count} tests pass)${NC}"

# ---- Step 3: npx tsc --noEmit (if frontend exists) ----
FRONTEND_DIR="crates/openscript-tauri/src/frontend"
if [ -d "$FRONTEND_DIR/node_modules" ]; then
  echo -n "[3/6] npx tsc --noEmit... "
  TSC_OUTPUT=$(cd "$FRONTEND_DIR" && npx tsc --noEmit 2>&1) || {
    echo -e "${RED}FAILED${NC}"
    echo "$TSC_OUTPUT" | tail -20
    FAIL "npx tsc --noEmit failed (TypeScript errors)"
  }
  echo -e "${GREEN}OK${NC}"
else
  echo -e "[3/7] npx tsc --noEmit... ${YELLOW}SKIPPED (no node_modules)${NC}"
fi

# ---- Step 4: workspace-lint (structure check, git-tracked files only) ----
echo -n "[4/7] workspace-lint (structure check)... "
if [ -f "workspace-lint.yaml" ]; then
  LINT_OUTPUT=$(python3 scripts/workspace-lint/workspace_lint.py 2>&1)
  LINT_WARNINGS=$(echo "$LINT_OUTPUT" | grep -c '\[warn\]' || true)
  if [ "$LINT_WARNINGS" -gt 0 ]; then
    echo -e "${RED}FAILED${NC}"
    echo "$LINT_OUTPUT" | grep '\[warn\]' | head -10
    FAIL "workspace-lint found ${LINT_WARNINGS} structure violations"
  fi
  echo -e "${GREEN}OK${NC}"
else
  echo -e "${YELLOW}SKIPPED (no workspace-lint.yaml)${NC}"
fi

# ---- Step 5: git status (no uncommitted changes) ----
echo -n "[5/7] git status (working tree clean)... "
UNCOMMITTED=$(git status --porcelain 2>&1)
if [ -n "$UNCOMMITTED" ]; then
  echo -e "${RED}FAILED${NC}"
  echo "$UNCOMMITTED"
  FAIL "uncommitted changes — you forgot to commit (run: git add -A && git commit)"
fi
echo -e "${GREEN}OK${NC}"

# ---- Step 6: git push origin main ----
echo -n "[6/7] git push origin main... "
PUSH_OUTPUT=$(git push origin main 2>&1) || {
  echo -e "${RED}FAILED${NC}"
  echo "$PUSH_OUTPUT"
  FAIL "git push failed — see AGENTS.md §7.5 for the push-failure hard-stop protocol"
}
echo -e "${GREEN}OK${NC}"

# ---- Step 7: git log origin/main..HEAD (nothing unpushed) ----
echo -n "[7/7] git log origin/main..HEAD (nothing unpushed)... "
UNPUSHED=$(git log origin/main..HEAD --oneline 2>&1)
if [ -n "$UNPUSHED" ]; then
  echo -e "${RED}FAILED${NC}"
  echo "$UNPUSHED"
  FAIL "unpushed commits — push them before starting the next iteration"
fi
echo -e "${GREEN}OK${NC}"

PASS
