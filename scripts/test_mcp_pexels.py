#!/usr/bin/env python3
"""End-to-end MCP test with Pexels API enabled (multi-broll backgrounds)."""
import json, subprocess, sys, time, os, re

os.chdir('/home/z/my-project/openscript')

proc = subprocess.Popen(
    ['./target/release/mcp-server'],
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    text=False, bufsize=0,
)

def send(msg):
    s = json.dumps(msg).encode()
    framed = f'Content-Length: {len(s)}\r\n\r\n'.encode() + s
    proc.stdin.write(framed)
    proc.stdin.flush()

def recv_want_id(want_id, timeout=240):
    t0 = time.time()
    while time.time() - t0 < timeout:
        header_buf = b''
        while b'\r\n\r\n' not in header_buf:
            b = proc.stdout.read(1)
            if not b:
                return None
            header_buf += b
        m = re.search(rb'content-length:\s*(\d+)', header_buf, re.I)
        n = int(m.group(1)) if m else 0
        if n == 0:
            continue
        body = b''
        while len(body) < n:
            chunk = proc.stdout.read(n - len(body))
            if not chunk:
                return None
            body += chunk
        msg = json.loads(body.decode())
        if msg.get('id') == want_id:
            return msg
        if msg.get('method') == 'notifications/progress':
            pp = msg.get('params', {})
            sys.stdout.write(f"  progress: {pp.get('progress')}/{pp.get('total')} - {pp.get('message','')}\n")
            sys.stdout.flush()
    return None

send({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}})
r = recv_want_id(1)
print("init ok:", r.get('result', {}).get('serverInfo', {}).get('name', 'unknown') if r else 'FAILED')
send({"jsonrpc":"2.0","method":"notifications/initialized"})

script_content = open('/home/z/my-project/test_pexels_script.json').read()
print("calling script.to_video with Pexels-enabled background...")
t0 = time.time()
send({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"script.to_video","arguments":{"script":script_content,"output_path":"/tmp/mcp_pexels_test.mp4","output_dir":"/tmp/mcp_pexels_artifacts"}}})
r = recv_want_id(2, timeout=240)
elapsed = time.time() - t0
print(f"elapsed: {elapsed:.1f}s")
if r is None:
    print("ERROR: timeout")
elif 'result' in r:
    content = r['result']['content'][0]['text']
    parsed = json.loads(content)
    print('status:', parsed.get('status'))
    print('output_path:', parsed.get('output_path'))
    print('total_duration_ms:', parsed.get('total_duration_ms'))
    print('warnings:', parsed.get('warnings'))
    if os.path.exists('/tmp/mcp_pexels_test.mp4'):
        sz = os.path.getsize('/tmp/mcp_pexels_test.mp4')
        print(f'MP4 file size: {sz} bytes ({sz/1024/1024:.2f} MB)')
elif 'error' in r:
    print('ERROR:', r['error'])
proc.terminate()
