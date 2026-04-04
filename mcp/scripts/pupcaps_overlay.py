#!/usr/bin/env python3
"""
pupcaps_overlay.py

Generate a styled caption overlay (MOV) using PupCaps and a retimed SRT aligned to an EDL.

Usage:
  python pupcaps_overlay.py --srt input.srt --edl edit.edl.json --css mcp/styles/pupcaps_center.css --width 1080 --height 1920 --fps 30 --animate --out captions.mov
"""

import argparse
import os
import shlex
import subprocess
from pathlib import Path


def run(cmd: str):
    print(f"[pupcaps_overlay] $ {cmd}")
    subprocess.run(cmd, shell=True, check=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--srt", required=True)
    ap.add_argument("--edl", required=True)
    ap.add_argument("--css", required=True)
    ap.add_argument("--width", type=int, default=1080)
    ap.add_argument("--height", type=int, default=1920)
    ap.add_argument("--fps", type=int, default=30)
    ap.add_argument("--animate", action="store_true")
    ap.add_argument("--out", required=True)
    args = ap.parse_args()

    base = Path(__file__).resolve().parent.parent
    scripts = base / "scripts"
    pupcaps_dir = base.parent / "third_party" / "PupCaps"

    # 1) Retimed SRT
    retimed = str(Path(args.out).with_suffix(".retimed.srt"))
    cmd_retime = f"python {shlex.quote(str(scripts / 'subs_retime.py'))} --srt {shlex.quote(args.srt)} --edl {shlex.quote(args.edl)} --out {shlex.quote(retimed)}"
    run(cmd_retime)

    # 2) PupCaps generate overlay
    pupcaps_bin = shlex.quote(str(pupcaps_dir / "pupcaps"))
    animate = " --animate" if args.animate else ""
    cmd_pup = f"node {pupcaps_bin} {shlex.quote(retimed)} --output {shlex.quote(args.out)} --width {args.width} --height {args.height} --fps {args.fps} --style {shlex.quote(args.css)}{animate}"
    run(cmd_pup)
    print(args.out)


if __name__ == "__main__":
    main()
