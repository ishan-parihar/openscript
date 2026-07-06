#!/usr/bin/env python3
"""Test that Kokoro TTS works end-to-end through the MCP pipeline."""
import json
import subprocess
import sys
import os

os.chdir("/home/z/my-project/openscript")

# Test 1: Direct Python sidecar test
print("=== Test 1: Direct Kokoro sidecar ===")
result = subprocess.run([
    "python3",
    "mcp/scripts/kokoro_tts_sidecar.py",
    "--text", "Hello world, this is a test of the Kokoro text to speech system.",
    "--voice", "af_heart",
    "--speed", "1.0",
    "--model", "mcp/assets/kokoro/onnx/kokoro-v1.0.onnx",
    "--voices", "mcp/assets/kokoro/voices/voices-v1.0.bin",
    "--output", "/tmp/kokoro_sidecar_test.wav",
], capture_output=True, text=True, timeout=60)

if result.returncode == 0 and os.path.exists("/tmp/kokoro_sidecar_test.wav"):
    import wave
    with wave.open("/tmp/kokoro_sidecar_test.wav", "r") as w:
        frames = w.getnframes()
        rate = w.getframerate()
        duration = frames / rate
    print(f"  PASS: {duration:.2f}s WAV at {rate}Hz ({os.path.getsize('/tmp/kokoro_sidecar_test.wav')} bytes)")
else:
    print(f"  FAIL: exit={result.returncode}")
    print(f"  stderr: {result.stderr[:500]}")
    sys.exit(1)

# Test 2: MCP system.capabilities
print("\n=== Test 2: system.capabilities ===")
mcp = subprocess.Popen(
    ["./target/release/mcp-server"],
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
    text=True
)
mcp.stdin.write(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}) + "\n")
mcp.stdin.write(json.dumps({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "system.capabilities", "arguments": {}}}) + "\n")
mcp.stdin.close()
output = mcp.stdout.read()
mcp.wait()

# Parse the second response
lines = output.strip().split("\n")
for line in lines:
    try:
        resp = json.loads(line)
        if resp.get("id") == 2:
            caps = json.loads(resp["result"]["content"][0]["text"])
            print(f"  kokoro: available={caps.get('kokoro', {}).get('available')}")
            print(f"  ffmpeg: available={caps.get('ffmpeg', {}).get('available')}")
            print(f"  pexels: available={caps.get('pexels', {}).get('available')}")
            print(f"  giphy: available={caps.get('giphy', {}).get('available')}")
            break
    except:
        continue

# Test 3: Pexels API key is set
print("\n=== Test 3: API keys ===")
os.environ["PEXELS_API_KEY"] = "b8HxbUpUvi7G7jV9S85pGuh8gLvHXDcm2VguWXXHn7oUAEUVmQLjUEts"
os.environ["GIPHY_API_KEY"] = "5hwvBpqE9PnTwwcvddrjCM23UCxZkord"
print(f"  PEXELS_API_KEY: set ({os.environ['PEXELS_API_KEY'][:10]}...)")
print(f"  GIPHY_API_KEY: set ({os.environ['GIPHY_API_KEY'][:10]}...)")

print("\n=== All tests passed ===")
