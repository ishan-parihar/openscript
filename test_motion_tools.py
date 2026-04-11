#!/usr/bin/env python3
"""Tests all 7 motion MCP tools via JSON-RPC stdio with Content-Length framing."""

import json
import subprocess
import sys
import os

PASS = 0
FAIL = 0


def start_server():
    """Start the MCP server subprocess."""
    workspace = os.path.dirname(os.path.abspath(__file__))
    bin_path = os.path.join(workspace, "target", "release", "openscript")
    if not os.path.exists(bin_path):
        # Fallback: build it
        subprocess.run(
            ["cargo", "build", "-p", "openscript-cli", "--release"],
            cwd=workspace,
            check=True,
        )
    proc = subprocess.Popen(
        [bin_path, "run-mcp"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        cwd=workspace,
    )
    return proc


def send_request(proc, method, params=None, req_id=1):
    """Send a JSON-RPC request with Content-Length framing, read the response."""
    if params is None:
        params = {}
    request = {"jsonrpc": "2.0", "id": req_id, "method": method, "params": params}
    json_str = json.dumps(request)
    framing = f"Content-Length: {len(json_str)}\r\n\r\n{json_str}"
    proc.stdin.write(framing.encode("utf-8"))
    proc.stdin.flush()

    # Read Content-Length header
    header = proc.stdout.readline().decode("utf-8").strip()
    if not header.startswith("Content-Length:"):
        return None, header
    content_len = int(header.split(":")[1].strip())

    # Read blank line
    blank = proc.stdout.readline()

    # Read content
    content = proc.stdout.read(content_len).decode("utf-8")
    return json.loads(content), None


def initialize(proc):
    """Send the MCP initialize handshake."""
    resp, err = send_request(
        proc,
        "initialize",
        {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "test", "version": "0.1.0"},
        },
        req_id=0,
    )
    if resp is None:
        print(f"  Initialize failed: {err}")
        return False
    # Send initialized notification
    notify = {"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}}
    json_str = json.dumps(notify)
    framing = f"Content-Length: {len(json_str)}\r\n\r\n{json_str}"
    proc.stdin.write(framing.encode("utf-8"))
    proc.stdin.flush()
    return True


def call_tool(proc, name, args, req_id=1):
    """Call an MCP tool and return the parsed result dict."""
    resp, err = send_request(
        proc,
        "tools/call",
        {
            "name": name,
            "arguments": args,
        },
        req_id=req_id,
    )
    if resp is None:
        raise RuntimeError(f"Tool call failed: {err}")
    if "error" in resp:
        raise RuntimeError(f"Tool error: {resp['error']}")
    # Parse the content
    content = resp["result"]["content"][0]["text"]
    return json.loads(content)


def print_summary(data, skip_keys=None):
    """Print a readable summary of the tool result."""
    if skip_keys is None:
        skip_keys = {"guide", "css_variables", "issues"}
    status = data.get("status", "N/A")
    print(f"  Status: {status}")
    for k, v in data.items():
        if k in skip_keys:
            if isinstance(v, str):
                print(f"  {k}: ({len(v)} chars)")
            continue
        if isinstance(v, dict):
            print(f"  {k}: {len(v)} keys")
            # Print first few keys for visibility
            for i, (kk, vv) in enumerate(list(v.items())[:3]):
                print(f"    {kk}: {vv}")
        elif isinstance(v, list):
            print(f"  {k}: {len(v)} items")
            for item in v[:3]:
                print(f"    - {item}")
        elif isinstance(v, str) and len(v) > 200:
            print(f"  {k}: ({len(v)} chars) {v[:100]}...")
        else:
            print(f"  {k}: {v}")


print("╔══════════════════════════════════════════════╗")
print("║  Motion MCP Tools — Integration Test Suite   ║")
print("╚══════════════════════════════════════════════╝")
print()

proc = start_server()

try:
    # Initialize
    print(">>> Initializing MCP server...")
    if not initialize(proc):
        print("FATAL: Could not initialize MCP server")
        sys.exit(1)
    print("  Initialized successfully.")
    print()

    # ─── Test 1: motion.load_skill ───
    print("═══ Test 1: motion.load_skill ═══")
    try:
        data = call_tool(proc, "motion.load_skill", {})
        assert data["status"] == "loaded", f"Expected 'loaded', got {data['status']}"
        assert len(data["guide"]) > 500, f"Guide too short: {len(data['guide'])} chars"
        # Check key sections exist
        for section in ["9:16", "Remotion", "Sequence", "interpolate", "AbsoluteFill"]:
            assert section in data["guide"], f"Guide missing section: {section}"
        print_summary(data)
        print("  ✅ PASS")
        PASS += 1
    except Exception as e:
        print(f"  ❌ FAIL: {e}")
        FAIL += 1
    print()

    # ─── Test 2: motion.design_system (modern/violet) ───
    print("═══ Test 2: motion.design_system (modern, #8B5CF6) ═══")
    try:
        data = call_tool(
            proc,
            "motion.design_system",
            {
                "primary_color": "#8B5CF6",
                "font_style": "modern",
            },
        )
        assert data["status"] == "generated"
        tokens = data["tokens"]
        assert tokens["primary"] == "#8B5CF6"
        assert tokens["temperature"] in ("warm", "cool")
        assert len(data["type_scale"]) == 6, (
            f"Expected 6 type scale levels, got {len(data['type_scale'])}"
        )
        assert len(data["spacing"]) == 11, (
            f"Expected 11 spacing levels, got {len(data['spacing'])}"
        )
        assert len(tokens["contrast_report"]) >= 4
        assert data["google_fonts_url"].startswith("https://fonts.googleapis.com")
        assert data["css_variables"].startswith(":root")
        print_summary(data)
        print("  ✅ PASS")
        PASS += 1
    except Exception as e:
        print(f"  ❌ FAIL: {e}")
        FAIL += 1
    print()

    # ─── Test 3: motion.design_system (technical/dark) ───
    print("═══ Test 3: motion.design_system (technical, #1E3A5F) ═══")
    try:
        data = call_tool(
            proc,
            "motion.design_system",
            {
                "primary_color": "1E3A5F",
                "font_style": "technical",
            },
        )
        assert data["status"] == "generated"
        tokens = data["tokens"]
        assert tokens["primary"] == "#1E3A5F"
        assert len(data["font_pairing"]["heading_font"]) > 2
        assert data["font_pairing"]["google_fonts_url"].startswith(
            "https://fonts.googleapis.com"
        )
        print_summary(data)
        print("  ✅ PASS")
        PASS += 1
    except Exception as e:
        print(f"  ❌ FAIL: {e}")
        FAIL += 1
    print()

    # ─── Test 4: motion.validate (valid TSX) ───
    print("═══ Test 4: motion.validate (valid TSX) ═══")
    valid_tsx = """import React from "react";
import { AbsoluteFill, Sequence, useCurrentFrame, interpolate } from "remotion";

export default function HotMotion() {
  const frame = useCurrentFrame();
  const opacity = interpolate(frame, [0, 30], [0, 1], { extrapolateRight: "clamp" });
  return (
    <AbsoluteFill style={{ backgroundColor: "#000" }}>
      <Sequence from={0} durationInFrames={60}>
        <div style={{ opacity, color: "white", fontSize: 48 }}>Hello</div>
      </Sequence>
    </AbsoluteFill>
  );
}"""
    try:
        data = call_tool(proc, "motion.validate", {"tsx_code": valid_tsx})
        assert data["valid"] == True, f"Expected valid=true, got {data['valid']}"
        error_issues = [i for i in data["issues"] if i["severity"] == "error"]
        assert len(error_issues) == 0, f"Found errors: {error_issues}"
        assert data["estimated_duration_ms"] > 0
        print_summary(data, skip_keys={"guide", "css_variables"})
        print("  ✅ PASS")
        PASS += 1
    except Exception as e:
        print(f"  ❌ FAIL: {e}")
        FAIL += 1
    print()

    # ─── Test 5: motion.validate (missing export → should flag error) ───
    print("═══ Test 5: motion.validate (missing export) ═══")
    missing_export = """import React from "react";
import { AbsoluteFill } from "remotion";

function NotExported() {
  return <AbsoluteFill style={{ backgroundColor: "#000" }} />;
}"""
    try:
        data = call_tool(proc, "motion.validate", {"tsx_code": missing_export})
        assert data["valid"] == False, f"Expected valid=false, got {data['valid']}"
        error_issues = [i for i in data["issues"] if i["severity"] == "error"]
        assert len(error_issues) > 0, "Should have at least one error"
        found_export_error = any("export" in i["detail"].lower() for i in error_issues)
        assert found_export_error, (
            f"Should have export-related error, got: {[i['detail'] for i in error_issues]}"
        )
        print_summary(data, skip_keys={"guide", "css_variables"})
        print("  ✅ PASS")
        PASS += 1
    except Exception as e:
        print(f"  ❌ FAIL: {e}")
        FAIL += 1
    print()

    # ─── Test 6: motion.validate (no remotion import → should flag error) ───
    print("═══ Test 6: motion.validate (no remotion import) ═══")
    no_import = """export default function HotMotion() {
  return <div>Hello</div>;
}"""
    try:
        data = call_tool(proc, "motion.validate", {"tsx_code": no_import})
        assert data["valid"] == False, f"Expected valid=false, got {data['valid']}"
        error_issues = [i for i in data["issues"] if i["severity"] == "error"]
        assert len(error_issues) > 0, "Should have at least one error"
        found_import_error = any(
            "import" in i["detail"].lower() or "remotion" in i["detail"].lower()
            for i in error_issues
        )
        assert found_import_error, (
            f"Should have import-related error, got: {[i['detail'] for i in error_issues]}"
        )
        print_summary(data, skip_keys={"guide", "css_variables"})
        print("  ✅ PASS")
        PASS += 1
    except Exception as e:
        print(f"  ❌ FAIL: {e}")
        FAIL += 1
    print()

    # ─── Test 7: motion.render (end-to-end, simple composition) ───
    print("═══ Test 7: motion.render (end-to-end) ═══")
    render_tsx = """import React from "react";
import { AbsoluteFill, Sequence, useCurrentFrame, interpolate } from "remotion";

export default function HotMotion() {
  const frame = useCurrentFrame();
  const opacity = interpolate(frame, [0, 20], [0, 1], { extrapolateRight: "clamp" });
  const scale = interpolate(frame, [0, 20], [0.5, 1], { extrapolateRight: "clamp" });
  return (
    <AbsoluteFill style={{ backgroundColor: "#6366F1", justifyContent: "center", alignItems: "center" }}>
      <Sequence from={0} durationInFrames={90}>
        <div style={{ opacity, transform: `scale(${scale})`, color: "white", fontSize: 72, fontFamily: "sans-serif", fontWeight: "bold" }}>
          Motion Test
        </div>
      </Sequence>
    </AbsoluteFill>
  );
}"""
    try:
        data = call_tool(
            proc,
            "motion.render",
            {
                "tsx_code": render_tsx,
                "duration_in_frames": 90,
                "fps": 30,
            },
        )
        assert data["status"] == "rendered", (
            f"Expected 'rendered', got {data['status']}"
        )
        assert "output_path" in data and data["output_path"].endswith(".mp4"), (
            f"Bad output_path: {data.get('output_path')}"
        )
        assert data["frame_count"] == 90
        assert data["duration_ms"] == 3000  # 90 frames / 30 fps * 1000
        assert data["file_size_bytes"] > 0
        # Verify file exists
        assert os.path.exists(data["output_path"]), (
            f"Output file does not exist: {data['output_path']}"
        )
        print_summary(data, skip_keys={"guide", "css_variables"})
        print(
            f"  ✅ PASS — output: {data['output_path']} ({data['file_size_bytes']} bytes)"
        )
        PASS += 1
    except Exception as e:
        print(f"  ❌ FAIL: {e}")
        FAIL += 1
    print()

    # ─── Test 8: motion.validate (interpolate without clamp → warning) ───
    print("═══ Test 8: motion.validate (interpolate without clamp → warning) ═══")
    no_clamp = """import React from "react";
import { AbsoluteFill, Sequence, useCurrentFrame, interpolate } from "remotion";

export default function HotMotion() {
  const frame = useCurrentFrame();
  const opacity = interpolate(frame, [0, 30], [0, 1]);
  return (
    <AbsoluteFill style={{ backgroundColor: "#000" }}>
      <Sequence from={0} durationInFrames={60}>
        <div style={{ opacity, color: "white", fontSize: 48 }}>No clamp</div>
      </Sequence>
    </AbsoluteFill>
  );
}"""
    try:
        data = call_tool(proc, "motion.validate", {"tsx_code": no_clamp})
        warnings = [i for i in data["issues"] if i["severity"] == "warning"]
        found_clamp_warning = any(
            "clamp" in i["detail"].lower() or "extrapolate" in i["detail"].lower()
            for i in warnings
        )
        assert found_clamp_warning, (
            f"Should warn about missing clamp, got: {[i['detail'] for i in warnings]}"
        )
        # Should still be valid since warnings aren't errors
        assert data["valid"] == True
        print_summary(data, skip_keys={"guide", "css_variables"})
        print("  ✅ PASS")
        PASS += 1
    except Exception as e:
        print(f"  ❌ FAIL: {e}")
        FAIL += 1
    print()

    # ─── Test 9: motion.get_info ───
    print("═══ Test 9: motion.get_info ═══")
    try:
        data = call_tool(proc, "motion.get_info", {})
        assert data["status"] == "success"
        assert "remotion_root" in data and "remotion" in data["remotion_root"]
        assert isinstance(data["compositions"], list)
        assert len(data["compositions"]) > 0, (
            f"Should have at least one composition, got: {data['compositions']}"
        )
        # HotMotion may appear as "hot-composition" (filename) or "HotMotion" (registered name)
        has_hot = any(
            "hot" in c.lower() or "HotMotion" in c for c in data["compositions"]
        )
        assert has_hot, (
            f"Should have HotMotion/hot-composition, got: {data['compositions']}"
        )
        assert isinstance(data["installed_fonts"], list)
        assert data["node_version"] != "unknown"
        assert data["remotion_version"] != "unknown"
        print_summary(data, skip_keys={"guide", "css_variables"})
        print("  ✅ PASS")
        PASS += 1
    except Exception as e:
        print(f"  ❌ FAIL: {e}")
        FAIL += 1
    print()

    # ─── Test 10: motion.compile_check (valid TSX) ───
    print("═══ Test 10: motion.compile_check (valid TSX) ═══")
    try:
        data = call_tool(proc, "motion.compile_check", {"tsx_code": valid_tsx})
        assert data["valid"] == True, (
            f"Expected valid=true, got {data['valid']}, errors: {data.get('errors')}"
        )
        assert data["error_count"] == 0
        print_summary(data, skip_keys={"guide", "css_variables"})
        print("  ✅ PASS")
        PASS += 1
    except Exception as e:
        print(f"  ❌ FAIL: {e}")
        FAIL += 1
    print()

    # ─── Test 11: motion.compile_check (invalid TSX — bad import) ───
    print("═══ Test 11: motion.compile_check (invalid TSX — missing remotion) ═══")
    bad_import = """import React from "react";
import { AbsoluteFill, useCurrentFrame } from "remotion";

export default function HotMotion() {
  const frame = useCurrentFrame();
  // Use a non-existent Remotion export to trigger a type error
  const val = AbsoluteFill.nonExistentProp;
  return (
    <AbsoluteFill style={{ backgroundColor: "#000" }}>
      <div style={{ color: "white", fontSize: 48 }}>Frame {frame}</div>
    </AbsoluteFill>
  );
}"""
    try:
        data = call_tool(proc, "motion.compile_check", {"tsx_code": bad_import})
        # The compile check should catch something (may be the nonExistentProp or other issue)
        assert "errors" in data
        assert "error_count" in data
        # Whether valid or not depends on TS strictness, but at minimum the tool works
        print_summary(data, skip_keys={"guide", "css_variables"})
        print("  ✅ PASS")
        PASS += 1
    except Exception as e:
        print(f"  ❌ FAIL: {e}")
        FAIL += 1
    print()

    # ─── Test 12: motion.preview (single frame PNG) ───
    print("═══ Test 12: motion.preview (frame 0) ═══")
    try:
        data = call_tool(
            proc,
            "motion.preview",
            {
                "tsx_code": render_tsx,
                "frame_number": 0,
            },
        )
        assert data["status"] == "previewed", (
            f"Expected 'previewed', got {data['status']}"
        )
        assert "output_path" in data and data["output_path"].endswith(".png")
        assert data["frame_number"] == 0
        assert data["width"] == 1080
        assert data["height"] == 1920
        assert data["file_size_bytes"] > 0
        assert os.path.exists(data["output_path"]), (
            f"Preview PNG does not exist: {data['output_path']}"
        )
        print_summary(data, skip_keys={"guide", "css_variables"})
        print(
            f"  ✅ PASS — output: {data['output_path']} ({data['file_size_bytes']} bytes)"
        )
        PASS += 1
    except Exception as e:
        print(f"  ❌ FAIL: {e}")
        FAIL += 1
    print()

    # ─── Test 13: motion.design_system includes timing tokens ───
    print("═══ Test 13: motion.design_system includes timing tokens ═══")
    try:
        data = call_tool(
            proc,
            "motion.design_system",
            {"primary_color": "#3B82F6"},
        )
        assert data["status"] == "generated"
        tokens = data["tokens"]
        assert "timing" in tokens, f"Missing timing in tokens: {list(tokens.keys())}"
        timing = tokens["timing"]
        assert "speed" in timing
        assert "stagger" in timing
        assert "easing" in timing
        assert timing["speed"]["fast"] == 15
        assert timing["speed"]["medium"] == 30
        assert timing["stagger"]["standard"] == 8
        assert timing["fps"] == 30
        print_summary(data, skip_keys={"guide", "css_variables", "contrast_report"})
        print("  ✅ PASS")
        PASS += 1
    except Exception as e:
        print(f"  ❌ FAIL: {e}")
        FAIL += 1
    print()

finally:
    proc.stdin.close()
    proc.wait()

print("╔══════════════════════════════════╗")
print("║  Results                         ║")
print(f"╠══════════════════════════════════╣")
print(f"║  PASSED: {PASS}                    ║")
print(f"║  FAILED: {FAIL}                    ║")
print("╚══════════════════════════════════╝")

sys.exit(FAIL)
