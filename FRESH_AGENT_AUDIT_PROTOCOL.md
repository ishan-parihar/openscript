# Fresh-Agent UX Audit Protocol

> **Purpose:** Standardized protocol for simulating how a brand-new AI agent experiences OpenScript's MCP tool surface. Run this after every major pipeline change to catch UX regressions.

---

## Quick Start (5-minute audit)

```bash
cd /home/ishanp/Documents/GitHub/MY-PROJECTS/CONTENT-CREATION/openscript

# 1. Build release binary
cargo build -p openscript-mcp --release --bin mcp-server

# 2. Verify server works
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' | ./target/release/mcp-server 2>/dev/null | head -c 500

# 3. Count tools
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' | ./target/release/mcp-server 2>/dev/null | python3 -c "import sys,json; print(json.load(sys.stdin)['result']['serverInfo'])"

# 4. List all tools
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' | timeout 10 ./target/release/mcp-server 2>/dev/null > /dev/null
echo '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' | timeout 10 ./target/release/mcp-server 2>/dev/null | python3 -c "import sys,json; [print(t['name']) for t in json.load(sys.stdin)['result']['tools']]"
```

---

## Full Audit Protocol

### Prerequisites

| Item | Value | Notes |
|------|-------|-------|
| MCP binary | `./target/release/mcp-server` | Must be release build |
| Audio source | `/home/ishanp/Downloads/audit_v3_render.mp4` | 135.4s Hindi speech |
| API keys | `~/.openscript/config.json` | Pexels, GIPHY configured |
| FFmpeg | `/usr/bin/ffmpeg` | n8.1.2 |
| Output dir | `/home/ishanp/Downloads/` | For rendered videos |

### Step 1: Agent Deployment

**Deploy with MINIMAL instructions.** The agent should only know:
1. Its role (e.g., "create a video from audio")
2. The MCP tool location (`./target/release/mcp-server`)
3. The input file path

**Do NOT give the agent:**
- Tool names or descriptions
- Workflow documentation
- Parameter examples
- Previous audit results

### Step 2: Discovery Phase

The agent should call:
1. `initialize` → get server info + instructions field
2. `tools/list` → get all tool definitions
3. Read tool descriptions to find the right tool

**Measure:** How many calls to find the right tool? (Ideal: 2 — initialize + tools/list)

### Step 3: Decision Phase

The agent should:
1. Scan tool descriptions for keywords matching its task
2. Select the optimal tool (one-call > multi-step)
3. Determine required parameters

**Measure:** Did the agent find the golden path tool? (e.g., `audio.to_video` for A2V)

### Step 4: Execution Phase

The agent should:
1. Call the selected tool with reasonable parameters
2. Wait for completion
3. Parse the response

**Measure:** Did the tool succeed? What's the output quality?

### Step 5: Verification Phase

The agent should:
1. Call verification tools on the output
2. Read scores and identify issues
3. Decide if re-render is needed

**Measure:** Can the agent complete the verification loop? What blocks it?

---

## MCP JSON-RPC Commands

### Initialize
```bash
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' | ./target/release/mcp-server 2>/dev/null
```

### List Tools
```bash
echo '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' | ./target/release/mcp-server 2>/dev/null
```

### Call a Tool
```bash
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"TOOL_NAME","arguments":{...}}}' | ./target/release/mcp-server 2>/dev/null
```

### Common Tool Calls

#### audio.to_video (A2V)
```bash
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"audio.to_video","arguments":{"audio_path":"/home/ishanp/Downloads/audit_v3_render.mp4","output_path":"/home/ishanp/Downloads/OUTPUT_NAME.mp4","aspect":"9:16","preset":"Balanced"}}}' | ./target/release/mcp-server 2>/dev/null
```

#### verify.audio
```bash
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"verify.audio","arguments":{"video_path":"VIDEO_PATH"}}}' | ./target/release/mcp-server 2>/dev/null
```

#### verify.production
```bash
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":12,"method":"tools/call","params":{"name":"verify.production","arguments":{"video_path":"VIDEO_PATH","timeline_path":"TIMELINE_PATH","min_grade":"D"}}}' | ./target/release/mcp-server 2>/dev/null
```

#### system.capabilities
```bash
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"system.capabilities","arguments":{}}}' | ./target/release/mcp-server 2>/dev/null
```

#### system.doctor
```bash
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"system.doctor","arguments":{}}}' | ./target/release/mcp-server 2>/dev/null
```

---

## Known Artifact Locations

After `audio.to_video` completes, artifacts are typically at:

| Artifact | Location Pattern |
|----------|-----------------|
| Output video | As specified in `output_path` |
| Timeline JSON | `/tmp/run{N}_output/timeline.json` |
| Timeline preview | `/tmp/run{N}_output/timeline_preview.txt` |
| SRT captions | Same dir as output video, `*.srt` |
| ASS captions | Same dir as output video, `*.ass` |
| Debug log | `{output_path}.debug.log` |
| Render manifest | `{output_path}.render_manifest.json` |

**Note:** Artifact paths are NOT returned by `audio.to_video` in its response. This is a known UX gap (see FRESH_AGENT_UX_AUDIT_20.md GAP #3).

---

## Scoring Rubric

| Category | Weight | What to Measure |
|----------|--------|-----------------|
| **Discovery** | 15% | Calls needed to find the right tool |
| **Decision** | 15% | Optimal tool selection (one-call vs multi-step) |
| **Execution** | 20% | Tool succeeds, reasonable parameters |
| **Verification** | 25% | Can complete verify loop, what blocks it |
| **Output Quality** | 25% | Production KPI grade, audio/caption scores |

### Grade Scale

| Grade | Score | Meaning |
|-------|-------|---------|
| A | 85-100 | Golden trajectory works end-to-end |
| B | 70-84 | Pipeline works, minor UX gaps |
| C | 55-69 | Pipeline works, significant gaps |
| D | 40-54 | Pipeline partially works, major gaps |
| F | <40 | Pipeline broken or unusable |

---

## Known UX Gaps (Track Across Audits)

| ID | Gap | Severity | Status | First Found |
|----|-----|----------|--------|-------------|
| GAP-1 | verify.captions requires srt_path (not returned by audio.to_video) | BLOCKER | Open | Audit #20 |
| GAP-2 | verify.production requires timeline_path (not returned by audio.to_video) | BLOCKER | Open | Audit #20 |
| GAP-3 | audio.to_video response lacks artifact paths | HIGH | Open | Audit #20 |
| GAP-4 | No preset enum in audio.to_video inputSchema | MEDIUM | Open | Audit #20 |
| GAP-5 | Production KPI D grade (54/100) | HIGH | Open | Audit #20 |

---

## Audit Output Template

```markdown
# Fresh-Agent UX Audit #[N] — [TRAJECTORY]

**Date:** YYYY-MM-DD
**MCP Server:** openscript-rs vX.Y.Z (N tools)
**Source:** [input file path]
**Output:** [output file path]

## Agent Simulation
[What the agent was given, what it discovered, what it decided]

## Tool Calls
[Sequential list of tool calls with inputs/outputs]

## Verification Scores
| Tool | Score | Status |
|------|-------|--------|
| verify.audio | X/100 | ✅/❌ |
| verify.captions | X/100 | ✅/❌ |
| verify.production | Grade (X/100) | ✅/❌ |

## UX Gaps Found
[New gaps or regressions from previous audits]

## Recommendations
[Prioritized list of fixes]
```

---

## Running the Audit via Subagent

To deploy a fresh agent via the Freebuff/Buffy system:

```
Deploy a subagent with only:
1. Its role: "Create a video from an audio file"
2. MCP tool location: "./target/release/mcp-server"
3. Audio file: "/home/ishanp/Downloads/audit_v3_render.mp4"

Then monitor:
- Tool discovery (how many calls to find the right tool)
- Tool selection (did it find audio.to_video?)
- Execution (did it succeed?)
- Verification (can it complete the verify loop?)
```

The subagent should be spawned with `tmux-cli` or `basher` agents that send JSON-RPC messages to the MCP server via stdin/stdout.
