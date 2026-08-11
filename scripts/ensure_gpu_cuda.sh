#!/usr/bin/env bash
# ensure_gpu_cuda.sh — Detect and self-heal the NVIDIA driver/library
# version mismatch so ORT's CUDAExecutionProvider actually works.
#
# WHY: A partial `pacman -Syu` (or a reboot after a module upgrade) leaves
# the LOADED nvidia kernel module at one version while the userspace libs
# (libcuda/nvidia-utils) are at another. nvidia-smi then fails with
# "Driver/library version mismatch", and onnxruntime silently reports
# "CUDA failure 803: GPU=-1" and falls back to CPU — turning minutes of
# TTS/render work into hours. This script detects that state and re-aligns
# the userspace packages to the loaded module, no reboot required.
#
# USAGE:
#   bash scripts/ensure_gpu_cuda.sh            # check + auto-heal
#   bash scripts/ensure_gpu_cuda.sh --check    # check only, print status
#   bash scripts/ensure_gpu_cuda.sh --force    # force re-alignment even if OK
#
# Exit codes: 0 = GPU usable; 1 = broken and NOT fixable (see output).
# Idempotent and safe: only touches nvidia userspace packages, never the
# kernel module, never needs a reboot, and never touches a live desktop
# session (running processes keep their in-memory libs).

set -u

MODE="${1:-auto}"            # auto | --check | --force
SUDO="${SUDO:-sudo}"
CACHE=/var/cache/pacman/pkg

say()  { printf '\033[1;34m[gpu]\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[gpu]\033[0m WARN: %s\n' "$*"; }
err()  { printf '\033[1;31m[gpu]\033[0m ERROR: %s\n' "$*" >&2; }

# ---------------------------------------------------------------------------
# 1. Is nvidia even present?
# ---------------------------------------------------------------------------
if ! command -v nvidia-smi >/dev/null 2>&1; then
    say "nvidia-smi not found — no NVIDIA userspace installed (CPU-only box?)."
    exit 0
fi

# ---------------------------------------------------------------------------
# 2. Does NVML initialize cleanly?
# ---------------------------------------------------------------------------
smi_out="$(nvidia-smi 2>&1 || true)"
if ! printf '%s' "$smi_out" | grep -qiE 'driver/library version mismatch|failed to initialize nvml'; then
    say "GPU healthy: nvidia-smi initializes."
    if [ "$MODE" = "--force" ]; then
        :
    else
        nvidia-smi --query-gpu=name,driver_version --format=csv,noheader 2>/dev/null | head -1 \
            | sed 's/^/  GPU: /'
        exit 0
    fi
fi

# ---------------------------------------------------------------------------
# 3. Mismatch (or forced). Resolve versions.
# ---------------------------------------------------------------------------
mod_ver="$(grep -oE 'NVRM version: NVIDIA[^ ]* [0-9]+\.[0-9]+\.[0-9]+' /proc/driver/nvidia/version 2>/dev/null \
           | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)"
mod_ver="${mod_ver:-$(cat /proc/driver/nvidia/version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)}"

if [ -z "$mod_ver" ]; then
    err "loaded nvidia kernel module version not found (/proc/driver/nvidia/version)."
    err "The module may not be loaded for the running kernel."
    exit 1
fi

# Installed userspace version (from the nvidia-utils package, if pacman).
usr_ver=""
if command -v pacman >/dev/null 2>&1; then
    usr_ver="$(pacman -Q nvidia-utils 2>/dev/null | awk '{print $2}' | cut -d- -f1)"
fi

say "loaded module : $mod_ver"
[ -n "$usr_ver" ] && say "userspace libs: $usr_ver"

if [ "$MODE" != "--force" ] && [ "$usr_ver" = "$mod_ver" ]; then
    say "already aligned — nothing to do."
    exit 0
fi

# ---------------------------------------------------------------------------
# 4. Find the matching userspace packages.
#    Prefer the pacman cache (offline, no network). Fall back to the package
#    manager if the exact version is unavailable locally.
# ---------------------------------------------------------------------------
if ! command -v pacman >/dev/null 2>&1; then
    err "non-pacman system; cannot auto-heal. Install nvidia userspace matching $mod_ver manually."
    exit 1
fi

pkgs=""
for p in nvidia-utils lib32-nvidia-utils nvidia-settings; do
    if ! pacman -Q "$p" >/dev/null 2>&1; then
        continue    # not installed — nothing to align
    fi
    cached="$(ls "$CACHE"/${p}-${mod_ver}-1-*.pkg.tar.zst 2>/dev/null | head -1)"
    if [ -n "$cached" ]; then
        pkgs="$pkgs $cached"
    else
        warn "no cached ${p}-${mod_ver}; will ask pacman (needs network)"
        pkgs="$pkgs $p"
    fi
done

if [ -z "$pkgs" ]; then
    err "no nvidia userspace packages installed to align."
    exit 1
fi

# ---------------------------------------------------------------------------
# 5. Apply. -Udd skips the dependency check so the module packages' pinned
#    nvidia-utils requirement doesn't block re-alignment to the loaded
#    module (the loaded module is ground truth, not the .PKGINFO pins).
# ---------------------------------------------------------------------------
say "aligning userspace to loaded module $mod_ver ..."
# shellcheck disable=SC2086
if [ "$MODE" = "--check" ]; then
    say "would run: $SUDO pacman -Udd --noconfirm$pkgs"
    exit 0
fi

if ! $SUDO pacman -Udd --noconfirm $pkgs 2>&1 | tail -6; then
    err "pacman failed to re-align userspace. Fix manually:"
    err "  sudo pacman -Udd --noconfirm $pkgs"
    exit 1
fi

# ---------------------------------------------------------------------------
# 6. Verify.
# ---------------------------------------------------------------------------
sleep 1
if nvidia-smi >/dev/null 2>&1; then
    say "GPU usable after alignment:"
    nvidia-smi --query-gpu=name,driver_version --format=csv,noheader 2>/dev/null | head -1 | sed 's/^/  /'
    exit 0
else
    err "alignment applied but nvidia-smi still fails. Check:"
    nvidia-smi 2>&1 | tail -2
    exit 1
fi
