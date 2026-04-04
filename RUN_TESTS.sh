#!/bin/bash
# OpenScript Timeline System - Comprehensive Test Suite
# Run this to verify all components are working

set -e

echo "======================================================================"
echo " OpenScript Timeline System - Comprehensive Test Suite"
echo "======================================================================"
echo ""

cd "$(dirname "$0")"

# Test 1: Unit Tests
echo "[1/4] Running unit tests..."
python3 mcp/test_implementation.py
echo "✓ Unit tests passed"
echo ""

# Test 2: End-to-End Workflow
echo "[2/4] Running end-to-end workflow test..."
python3 mcp/test_e2e_workflow.py
echo "✓ End-to-end test passed"
echo ""

# Test 3: Verify Asset Indexes
echo "[3/4] Verifying asset indexes..."
if [ -f "mcp/assets/sfx_index.json" ]; then
    SFX_COUNT=$(python3 -c "import json; print(len(json.load(open('mcp/assets/sfx_index.json'))['assets']))")
    echo "✓ SFX index: $SFX_COUNT assets"
else
    echo "✗ SFX index not found"
    exit 1
fi

if [ -f "mcp/assets/music_index.json" ]; then
    MUSIC_COUNT=$(python3 -c "import json; print(len(json.load(open('mcp/assets/music_index.json'))['assets']))")
    echo "✓ Music index: $MUSIC_COUNT assets"
else
    echo "✗ Music index not found"
    exit 1
fi
echo ""

# Test 4: MCP Server Validation
echo "[4/4] Validating MCP server..."
if [ -f "mcp/reels_mcp_server_v2.py" ]; then
    echo "✓ MCP server v2 exists"
    # Count available tools
    TOOL_COUNT=$(python3 -c "
import sys
sys.path.insert(0, 'mcp')
from reels_mcp_server_v2 import tool_list_all
tools = tool_list_all()
print(len(tools))
")
    echo "✓ MCP server exposes $TOOL_COUNT tools"
else
    echo "✗ MCP server v2 not found"
    exit 1
fi
echo ""

echo "======================================================================"
echo " All Tests Passed!"
echo "======================================================================"
echo ""
echo "Summary:"
echo "  - Unit tests: PASSED"
echo "  - E2E workflow: PASSED"
echo "  - Asset indexes: VERIFIED"
echo "  - MCP server: VALIDATED"
echo ""
echo "The OpenScript Timeline System is ready for use!"
echo ""
echo "Next steps:"
echo "  1. Review VERIFICATION_COMPLETE.md for full documentation"
echo "  2. Check mcp/QUICKSTART.md for usage examples"
echo "  3. Start using the MCP server: python3 mcp/reels_mcp_server_v2.py"
echo ""
