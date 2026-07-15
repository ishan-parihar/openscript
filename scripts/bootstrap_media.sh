#!/usr/bin/env bash
# bootstrap_media.sh — Media/asset doctor + optional first-run indexes.
#
# Ensures a machine can produce production-grade stock (not gradients):
#   - ~/.openscript/config.json (merge env keys)
#   - ffmpeg / yt-dlp
#   - live probe Pexels / GIPHY when keys present
#   - optional library.build for tagged music
#
# Usage:
#   bash scripts/bootstrap_media.sh
#   bash scripts/bootstrap_media.sh --with-library
#   bash scripts/bootstrap_media.sh --probe-only
#   PEXELS_API_KEY=... GIPHY_API_KEY=... bash scripts/bootstrap_media.sh
#
# Never prints secret values.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

WITH_LIBRARY=0
PROBE_ONLY=0
for arg in "$@"; do
  case "$arg" in
    --with-library) WITH_LIBRARY=1 ;;
    --probe-only) PROBE_ONLY=1 ;;
    -h|--help)
      sed -n '2,20p' "$0" | sed 's/^# \?//'
      exit 0
      ;;
    *) echo "Unknown arg: $arg" >&2; exit 1 ;;
  esac
done

ok() { echo "  [✓] $1"; }
bad() { echo "  [✗] $1"; }
warn() { echo "  [!] $1"; }

echo "=== OpenScript media bootstrap ==="

# 1) Config
bash "$REPO_ROOT/scripts/setup_openscript_config.sh" || true
CONFIG="${OPENSCRIPT_CONFIG_DIR:-$HOME/.openscript}/config.json"

# 2) Binaries
echo ""
echo "Binaries"
if command -v ffmpeg >/dev/null 2>&1; then ok "ffmpeg"; else bad "ffmpeg missing"; fi
if command -v ffprobe >/dev/null 2>&1; then ok "ffprobe"; else bad "ffprobe missing"; fi
if command -v yt-dlp >/dev/null 2>&1; then ok "yt-dlp"; else bad "yt-dlp missing (YouTube fallback)"; fi

# 3) Key + live probes (Python, no secret echo)
echo ""
echo "API keys + live probes"
python3 <<'PY'
import json, os, sys, urllib.request
from pathlib import Path

config_path = Path(os.environ.get("OPENSCRIPT_CONFIG_DIR", Path.home() / ".openscript")) / "config.json"
keys = {}
if config_path.exists():
    try:
        cfg = json.loads(config_path.read_text())
        keys = cfg.get("api_keys") or {}
    except Exception as e:
        print(f"  [✗] config parse: {e}")
        sys.exit(0)

def has(k):
    return bool((keys.get(k) or "").strip())

# Cloudflare (error 1010) blocks Python-urllib's default UA — always set one.
_UA = "OpenScript/1.0 (media-bootstrap; +https://github.com/ishan-parihar/openscript)"

def probe_pexels(key):
    req = urllib.request.Request(
        "https://api.pexels.com/videos/search?query=office+desk&per_page=1&orientation=portrait",
        headers={
            "Authorization": key,
            "User-Agent": _UA,
            "Accept": "application/json",
        },
    )
    with urllib.request.urlopen(req, timeout=20) as r:
        data = json.loads(r.read().decode())
    return bool(data.get("videos"))

def probe_giphy(key):
    url = f"https://api.giphy.com/v1/stickers/search?api_key={key}&q=desk&limit=1&rating=g"
    req = urllib.request.Request(url, headers={"User-Agent": _UA, "Accept": "application/json"})
    with urllib.request.urlopen(req, timeout=20) as r:
        data = json.loads(r.read().decode())
    return bool(data.get("data"))

ready = True
for name, env in [("pexels", "PEXELS_API_KEY"), ("giphy", "GIPHY_API_KEY"), ("pixabay", "PIXABAY_API_KEY"), ("openrouter", "OPENROUTER_API_KEY")]:
    if has(name):
        print(f"  [✓] {name} key present in config")
    elif os.environ.get(env):
        print(f"  [✓] {name} key present in env (re-run setup_openscript_config.sh to persist)")
    else:
        print(f"  [✗] {name} key missing")
        if name in ("pexels",):
            ready = False

# Live probes
if has("pexels"):
    try:
        if probe_pexels(keys["pexels"]):
            print("  [✓] pexels live search OK")
        else:
            print("  [!] pexels live search returned 0 videos")
            ready = False
    except Exception as e:
        print(f"  [✗] pexels live probe failed: {type(e).__name__}")
        ready = False

if has("giphy"):
    try:
        if probe_giphy(keys["giphy"]):
            print("  [✓] giphy live search OK")
        else:
            print("  [!] giphy live search empty")
    except Exception as e:
        print(f"  [✗] giphy live probe failed: {type(e).__name__}")

# Indexes
print("")
print("Local indexes")
prod = Path("mcp/assets/music_production/index.json")
if prod.exists():
    print("  [✓] music_production pack (cold-start beds)")
else:
    print("  [✗] music_production pack missing")
    ready = False

lib = Path("mcp/assets/music_library_index.json")
if lib.exists():
    print("  [✓] music_library_index.json (optional large index)")
else:
    print("  [!] music_library_index.json missing — optional: bootstrap_media.sh --with-library")

sfx = Path("mcp/assets/sfx_index.json")
sfx_pack = Path("mcp/assets/sfx_pack")
if sfx.exists():
    print("  [✓] sfx_index.json")
    try:
        d = json.loads(sfx.read_text())
        assets = d.get("assets") or []
        missing = 0
        ok_n = 0
        for a in assets[:50]:
            p = a.get("path") or ""
            if not p:
                continue
            if Path(p).exists():
                ok_n += 1
            else:
                missing += 1
        if missing and ok_n == 0:
            print(f"  [!] {missing}/sampled SFX paths missing (use portable sfx_pack)")
            if not sfx_pack.is_dir():
                ready = False
        elif missing:
            print(f"  [!] {missing} missing, {ok_n} resolvable in sample")
        else:
            print(f"  [✓] SFX paths resolve ({ok_n} sampled)")
    except Exception:
        pass
elif sfx_pack.is_dir():
    print("  [!] sfx_index.json missing but sfx_pack present — run sfx.index")
else:
    print("  [✗] no SFX pack/index")
    ready = False

kokoro = Path("mcp/assets/kokoro/onnx/kokoro-v1.0.onnx")
if kokoro.exists():
    print("  [✓] Kokoro model")
else:
    print("  [✗] Kokoro model missing — run setup.sh")
    ready = False

# Pexels key required for production_ready
if not has("pexels") and not os.environ.get("PEXELS_API_KEY"):
    ready = False

print("")
if ready:
    print("production_ready: YES (keys + portable packs look good)")
else:
    print("production_ready: NO — fix items marked [✗]")
    print("See docs/INSTALL.md and docs/INSTALL_MEDIA_DEPS_PLAN.md")
PY

if [[ "$PROBE_ONLY" -eq 1 ]]; then
  exit 0
fi

if [[ "$WITH_LIBRARY" -eq 1 ]]; then
  echo ""
  echo "=== library.build (may take ~2 min) ==="
  if [[ -x ./target/release/mcp-server ]]; then
    python3 <<'PY'
import json, subprocess, sys
from pathlib import Path
bin_path = Path("target/release/mcp-server")
if not bin_path.exists():
    print("mcp-server not built; skip library.build")
    sys.exit(0)
proc = subprocess.Popen(
    [str(bin_path)],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.DEVNULL,
    text=True,
)
def call(method, params, id_):
    proc.stdin.write(json.dumps({"jsonrpc": "2.0", "id": id_, "method": method, "params": params}) + "\n")
    proc.stdin.flush()
    return json.loads(proc.stdout.readline())
try:
    call("initialize", {}, 1)
    r = call("tools/call", {"name": "library.build", "arguments": {}}, 2)
    text = r.get("result", {}).get("content", [{}])[0].get("text", "{}")
    print(text[:1500])
except Exception as e:
    print("library.build failed:", e)
finally:
    proc.terminate()
PY
  else
    echo "Build release mcp-server first: cargo build -p openscript-mcp --release"
  fi
fi

echo ""
echo "Done. Next: director.run with a 5-scene script and check verify.production."
