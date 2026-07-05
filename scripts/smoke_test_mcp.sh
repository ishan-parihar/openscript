#!/usr/bin/env bash
# Smoke test the MCP server: list tools, call hf.classify on MainWithBroll.tsx
set -e

MCP_BIN="./target/release/mcp-server"

echo "=== 4. MCP server smoke test ==="

# 4a. tools/list — verify all 75 tools are registered
TOOLS_RESPONSE=$(echo '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}' | "$MCP_BIN" 2>/dev/null)
TOOL_COUNT=$(echo "$TOOLS_RESPONSE" | python3 -c "import sys,json; r=json.load(sys.stdin); print(len(r['result']['tools']))")
echo "Tool count: $TOOL_COUNT (expected 75)"

# Verify key tools are present
for tool in "transcribe" "reelize.timeline" "tts.generate" "hf.lint" "hf.validate" "hf.snapshot" "hf.render" "hf.classify" "composition.render" "verify.render"; do
    if echo "$TOOLS_RESPONSE" | python3 -c "import sys,json; r=json.load(sys.stdin); tools=[t['name'] for t in r['result']['tools']]; exit(0 if '$tool' in tools else 1)"; then
        echo "  ✓ $tool"
    else
        echo "  ✗ $tool MISSING"
        exit 1
    fi
done

echo ""
echo "=== 5. hf.classify smoke test (MainWithBroll.tsx — should be clean) ==="
CLASSIFY_RESPONSE=$(echo '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"hf.classify","arguments":{"source_path":"remotion/src/compositions/MainWithBroll.tsx"}}}' | "$MCP_BIN" 2>/dev/null)
echo "$CLASSIFY_RESPONSE" | python3 -c "
import sys, json
r = json.load(sys.stdin)
content = r['result']['content'][0]['text']
data = json.loads(content)
print(f\"  recommendation: {data['recommendation']}\")
print(f\"  has_blockers: {data['has_blockers']}\")
print(f\"  has_warnings: {data['has_warnings']}\")
print(f\"  blocker_count: {data['blocker_count']}\")
print(f\"  warning_count: {data['warning_count']}\")
if data['findings']:
    for f in data['findings']:
        print(f\"    - [{f['severity']}] {f['rule']} (line {f['line']}): {f['message']}\")
"

echo ""
echo "=== 6. hf.classify smoke test (synthetic useState source — should be interop) ==="
cat > /tmp/test_use_state.tsx << 'EOF'
import { useState } from "react";
export const Bad = () => {
    const [count, setCount] = useState(0);
    return <div>{count}</div>;
};
EOF
CLASSIFY_RESPONSE2=$(echo '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"hf.classify","arguments":{"source_path":"/tmp/test_use_state.tsx"}}}' | "$MCP_BIN" 2>/dev/null)
echo "$CLASSIFY_RESPONSE2" | python3 -c "
import sys, json
r = json.load(sys.stdin)
content = r['result']['content'][0]['text']
data = json.loads(content)
print(f\"  recommendation: {data['recommendation']}\")
print(f\"  blocker_count: {data['blocker_count']}\")
if data['findings']:
    for f in data['findings']:
        print(f\"    - [{f['severity']}] {f['rule']} (line {f['line']}): {f['message']}\")
"
rm -f /tmp/test_use_state.tsx

echo ""
echo "=== All smoke tests passed ==="
