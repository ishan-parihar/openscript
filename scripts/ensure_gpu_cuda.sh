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
#
# NOTE: if sudo is not passwordless, the alignment step will prompt for a
# password — run it in a terminal you can interact with.

set -u
set -o pipefail

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
# 2. Does NVML initialize cleanly? Treat ONLY a clean query as healthy —
#    any other NVML failure (mismatch, no devices, driver error) is suspect.
# ---------------------------------------------------------------------------
if smi_out="$(nvidia-smi --query-gpu=name,driver_version --format=csv,noheader 2>&1)" \
   && printf '%s' "$smi_out" | grep -q 'NVIDIA-SMI\|, [0-9]'; then
    say "GPU healthy: nvidia-smi initializes."
    if [ "$MODE" = "--force" ]; then
        :
    else
        printf '  GPU: %s\n' "$(printf '%s' "$smi_out" | head -1)"
        exit 0
    fi
fi

# ---------------------------------------------------------------------------
# 3. Mismatch (or forced). Resolve versions.
# ---------------------------------------------------------------------------
# /proc/driver/nvidia/version line:
#   NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  610.43.03  Release Build
# Anchor extraction to the NVRM line, then take its first dotted triple
# (the version) — the anchored grep keeps other lines' numbers (GCC, etc.)
# from winning.
mod_ver="$(grep -E 'NVRM version:' /proc/driver/nvidia/version 2>/dev/null \
           | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)"

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

if ! command -v pacman >/dev/null 2>&1; then
    err "non-pacman system; cannot auto-heal. Install nvidia userspace matching $mod_ver manually."
    exit 1
fi

# ---------------------------------------------------------------------------
# 4. Resolve the matching userspace packages — as FILE PATHS only.
#    Prefer the pacman cache (offline). If any package is missing from the
#    cache, fall back to a version-pinned `pacman -S` (network) — pacman -U
#    requires file paths, so a bare name would silently fail.
# ---------------------------------------------------------------------------
cached_pkgs=""
need_net=0
for p in nvidia-utils lib32-nvidia-utils nvidia-settings; do
    if ! pacman -Q "$p" >/dev/null 2>&1; then
        continue    # not installed — nothing to align
    fi
    cached="$(ls "$CACHE"/${p}-${mod_ver}-*-*.pkg.tar.zst 2>/dev/null | head -1)"
    if [ -n "$cached" ]; then
        cached_pkgs="$cached_pkgs $cached"
    else
        warn "no cached ${p}-${mod_ver}; will fetch via pacman (needs network)"
        need_net=1
    fi
done

if [ "$MODE" = "--check" ]; then
    if [ "$need_net" = "1" ]; then
        say "would run: $SUDO pacman -Sdd --needed nvidia-utils=$mod_ver lib32-nvidia-utils=$mod_ver nvidia-settings=$mod_ver"
    else
        say "would run: $SUDO pacman -Udd --noconfirm$cached_pkgs"
    fi
    exit 0
fi

say "aligning userspace to loaded module $mod_ver ..."
rc=0
if [ "$need_net" = "1" ]; then
    # Version-pinned network fetch (matches the loaded module exactly).
    $SUDO pacman -Sdd --needed --noconfirm \
        "nvidia-utils=$mod_ver" "lib32-nvidia-utils=$mod_ver" "nvidia-settings=$mod_ver" 2>&1 | tail -6
    rc=${PIPESTATUS[0]}
elif [ -n "$cached_pkgs" ]; then
    # Offline re-alignment from cache (-Udd skips the module packages' pinned
    # nvidia-utils requirement; the loaded module is ground truth).
    $SUDO pacman -Udd --noconfirm $cached_pkgs 2>&1 | tail -6
    rc=${PIPESTATUS[0]}
fi

if [ "$rc" -ne 0 ]; then
    err "pacman failed to re-align userspace (exit $rc). Fix manually:"
    if [ "$need_net" = "1" ]; then
        err "  sudo pacman -Sdd --needed nvidia-utils=$mod_ver lib32-nvidia-utils=$mod_ver nvidia-settings=$mod_ver"
    else
        err "  sudo pacman -Udd --noconfirm $cached_pkgs"
    fi
    exit 1
fi

# ---------------------------------------------------------------------------
# 5. Verify.
# ---------------------------------------------------------------------------
sleep 1
if smi_out="$(nvidia-smi --query-gpu=name,driver_version --format=csv,noheader 2>&1)" \
   && printf '%s' "$smi_out" | grep -q 'NVIDIA-SMI\|, [0-9]'; then
    say "GPU usable after alignment:"
    printf '  %s\n' "$(printf '%s' "$smi_out" | head -1)"
    exit 0
else
    err "alignment applied but nvidia-smi still fails:"
    nvidia-smi 2>&1 | tail -2
    exit 1
fi
