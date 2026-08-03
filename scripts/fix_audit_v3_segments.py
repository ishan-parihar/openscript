import json
import os

path = "test_fixtures/audit_v3_render_audio.timeline.json"
with open(path, "r") as f:
    tl = json.load(f)

segments = tl["segments"]
old_broll = tl["tracks"].get("broll", [])
new_broll = []

# Mapping from time (ms) to broll event
def find_broll(time_ms):
    for evt in old_broll:
        if evt["start_ms"] <= time_ms < evt["end_ms"]:
            return evt
    return None

for i, seg in enumerate(segments):
    start_ms = int(seg["start"] * 1000)
    end_ms = int(seg["end"] * 1000)
    mid_ms = (start_ms + end_ms) // 2
    
    match = find_broll(mid_ms)
    if not match:
        continue
        
    evt = match.copy()
    evt["id"] = f"broll_{i+1:03}"
    evt["start_ms"] = start_ms
    evt["end_ms"] = end_ms
    # Set duration_ms to match segment
    evt["duration_ms"] = end_ms - start_ms
    new_broll.append(evt)

tl["tracks"]["broll"] = new_broll
# Production mode: zoompan + looping + PTS alignment are what the render
# pipeline must exercise. Raw render mode (raw_render=true) is only for
# segmentation-correctness audits and strips the very post-processing the
# user wants back — do not force it on the regression fixture.
tl["raw_render"] = False

# Also update assets mapping — the existing ones are fine
# because we kept the asset_id.

with open(path, "w") as f:
    json.dump(tl, f, indent=2)

print(f"Fixed {len(new_broll)} b-roll events (one per segment). Production mode (raw_render=false).")
