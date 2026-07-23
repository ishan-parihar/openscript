# AGENTS.md — OpenScript Development Protocol

> This document is the canonical development protocol for the OpenScript repository.
> Every agent (human or AI) that touches this codebase MUST follow the rules below.
> The companion [`AGENT_GUIDE.md`](./AGENT_GUIDE.md) covers the MCP *tool* surface
> for runtime agents; this document covers the *engineering* rules for development
> agents.

> ### ⚠️ IRON RULE — ALWAYS PUSH COMMITS AFTER EACH ITERATION
>
> **Every iteration MUST end with `git commit` AND `git push origin main`.**
> No exceptions. No batching. No "I'll push at the end." If the session crashes
> with unpushed commits, that work is gone. Push immediately after every
> verified iteration. See [§7](#7-git-workflow) for the full protocol.

---

## 1. Repository Layout

```
openscript/
├── crates/                     # Rust workspace (8 crates)
│   ├── openscript-core/        #   types, timeline schema, SRT, captions, amplitude, transcript analysis
│   ├── openscript-ffmpeg/      #   ffmpeg filter graph, render, multilayer render, subtitles
│   ├── openscript-tts/         #   Kokoro TTS client + voice profile registry
│   ├── openscript-transcribe/  #   Apex transcription (whisper_timestamped wrapper (Apex transcription — stays))
│   ├── openscript-assets/      #   Pexels client, music index, SFX index
│   ├── openscript-mcp/         #   MCP server + 70 tool handlers (tools.rs + hf.rs)
│   ├── openscript-cli/         #   CLI (clap) — thin wrapper over route_tool()
│   ├── openscript-tauri/       #   Tauri desktop app (Rust commands + React frontend)
│   └── openscript-ui/          #   Legacy TUI (ratatui) — minimal maintenance
├── mcp/
│   ├── scripts/                # Python ML sidecars (Kokoro, Whisper, Apex, music indexer)
│   ├── assets/                 # Committed binary assets + generated JSON indices
│   └── styles/                 # PupCaps caption CSS
├── hyperframes/                # Default render engine (HTML + GSAP) + interop template
├── remotion/                   # Escape-hatch render engine (React + Remotion v4)
├── skills/                     # Dev-reference docs for HyperFrames + Remotion translation
├── scripts/                    # smoke_test_mcp.sh and other shell helpers
├── AGENT_GUIDE.md              # MCP tool catalog for runtime agents
├── AGENTS.md                   # THIS FILE — engineering protocol
└── TOOL_AUDIT_REPORT.md        # Historical audit (2026-04-11)
```

### Crate dependency graph (must not form cycles)

```
openscript-ui ──┐
openscript-cli ─┤
openscript-tauri┼──> openscript-mcp ──> openscript-ffmpeg ──> openscript-core
                │                  └──> openscript-assets  ──┘
                │                  └──> openscript-tts     ──┘
                │                  └──> openscript-transcribe ──┘
                └──> openscript-core (direct, for state types)
```

**Iron rule:** `openscript-core` depends on NOTHING in this workspace. It is the
leaf types crate. `openscript-ffmpeg` depends only on `openscript-core`. The
MCP crate is the integrator; nothing depends on it except the binaries (CLI,
Tauri, MCP server).

---

## 2. The Golden Trajectory

OpenScript has exactly **two supported entry points** for creating a video. All
other tools exist to support these trajectories or for NLE editing of existing
footage.

### Trajectory A — From-scratch creation (default)

```
script.parse  →  script.to_video
```

`script.to_video` is a one-call orchestrator that internally runs:
`script.generate_voices` → `script.build_captions` → `background.fetch` (per
scene, unique Pexels clips) → `sticker.render` (GIPHY per speaker) →
`script.to_timeline` → `multilayer_render`.

**Do not** chain the intermediate tools manually unless the user explicitly asks
for fine-tuning. `script.to_video` is the golden path.

### Trajectory B — NLE editing of existing footage

```
transcribe  →  timeline.build  →  broll.director  →  timeline.render
```

For editing an existing video (cut, caption, b-roll, music) without
from-scratch TTS.

### Trajectory C — From Audio File

```
transcribe → srt.prepare → timeline.build → timeline.add_segment → broll.director → library.search → music.assign → sfx.assign → timeline.validate → timeline.render → verify.production
```

`audio.to_video` was a monolithic orchestrator — it was deleted because the **agent** should decide the tool sequence, not hardcoded Rust. The atomic tools above give the agent full control.

### Trajectory D — From Existing Video (NLE + Re-edit)

```
transcribe → srt.prepare → reelize.brief → (agent decides segments) → timeline.build → timeline.add_segment → broll.director → library.search → music.assign → sfx.assign → timeline.validate → timeline.render → verify.production
```

`video.to_reel` was deleted for the same reason — agent orchestration is superior to hardcoded pipelines.

### Discovery trajectory (call first when unsure)

```
system.capabilities  →  help.tool
```

`system.capabilities` probes which subsystems are wired (ffmpeg, Kokoro, Pexels,
GIPHY, transcription, etc.). `help.tool` finds the right tool for a
natural-language task description. **Always call `system.capabilities` first
in a new environment** — it tells you which downstream tools will work.

---

## 3. Code Style

### Rust

- **Edition 2021.** Workspace deps in the root `Cargo.toml`; crate-specific deps
  in each crate's `Cargo.toml`.
- **No `unwrap()` / `expect()` / `panic!()` in production code.** Use `?` and
  `thiserror`. Tests are the only exception.
- **No `unsafe`** without a SAFETY comment and reviewer sign-off.
- **Error types** derive `thiserror::Error` with `#[error("...")]` messages.
  Never `Box<dyn Error>` — always a concrete enum.
- **Async** uses `tokio`. MCP handlers return
  `Pin<Box<dyn Future<Output = Result<Value, ToolError>> + Send + '_>>` via
  `Box::pin(async move { ... })` because the dispatcher needs a single return
  type across all handlers.
- **Serde** uses `#[serde(default)]` and `#[serde(default = "fn_name")]` for
  backward-compatible schema evolution. Never rename a field without keeping
  a `#[serde(alias = "old_name")]` for one release cycle.
- **`tracing`** for all logging, not `println!`/`eprintln!`. Use
  `tracing::warn!`, `tracing::info!`, `tracing::debug!`.
- **Tests** live in `#[cfg(test)] mod tests` at the bottom of each file.
  Integration tests go in `crates/<crate>/tests/`.

### TypeScript (Tauri frontend)

- **Strict mode** (`"strict": true` in `tsconfig.json`). No `any` without a
  comment explaining why.
- **`noUnusedLocals` + `noUnusedParameters`** are on. Delete unused code; do
  not prefix with `_` unless the unused symbol is an interface conformance.
- **Stores** use `zustand`. One store per concern (`render`, `transcript`,
  `voice`, `assets`, `editor`, `project`, `ai`).
- **Tauri invoke wrappers** live in `lib/tauri.ts`. Every Rust `#[tauri::command]`
  gets a typed wrapper. The Rust command name is `snake_case`; the TS wrapper
  name is `camelCase`.
- **No `console.log` in committed code.** Use the store's error field or a
  toast.

### Python (ML sidecars)

- **Python 3.11+** for all sidecars.
- **`subprocess.run(..., shell=False)`** always. If you must use `shell=True`,
  `shlex.quote` every interpolation.
- **Stdin/stdout JSON protocol** for long-lived sidecars. Fresh-process-per-call
  is acceptable for one-shot tools but NEVER for per-segment TTS or alignment.
  (Note: `music_library_indexer.py` was deleted in Phase C — its `--build` path
  is now a native Rust module at `crates/openscript-mcp/src/library_indexer.rs`.The 2 remaining Python sidecars — `kokoro_tts_sidecar.py` and `whisper_align.py` — wrap Python-only ML models and must stay Python. The HinglishGgml transcription uses `hinglish_ggml_transcriber.py` (whisper.cpp wrapper).)
- **No `os.system`**, no `eval`, no `exec`.

---

## 4. Adding a New MCP Tool

Checklist for adding a tool to `crates/openscript-mcp/src/tools.rs`:

1. **Add the tool definition** to the JSON array in `tool_definitions()`. Include
   `name`, `description` (one sentence + "Returns: ..."), and `inputSchema`
   with `type: "object"`, `properties`, `required`, `additionalProperties: false`.
2. **Add the route** to `route_tool()`:
   ```rust
   "your.tool.name" => Box::pin(handle_your_tool(args)),
   ```
3. **Write the handler** as `async fn handle_your_tool(args: Value) -> Result<Value, ToolError>`.
   Use `extract_str`/`default_str`/`default_u32`/etc. helpers — never index
   `args["key"]` directly (panics on missing keys).
4. **Update the tool count** in:
   - `server.rs::handle_initialize` instructions string ("N tools")
   - `AGENT_GUIDE.md` Tool Taxonomy header
   - `crates/openscript-mcp/tests/integration_test.rs::test_mcp_tools_list` assertion
   - `scripts/smoke_test_mcp.sh` expected count comment
5. **Add an integration test** in `integration_test.rs` that calls the tool via
   the MCP protocol and asserts on the response shape.
6. **Document the tool** in `AGENT_GUIDE.md` with a "When to use" table row.
7. **If the tool needs a Tauri frontend wrapper**, add the Rust
   `#[tauri::command]` in `commands/<area>.rs`, register it in `main.rs`'s
   `invoke_handler!` macro, add the TS wrapper in `lib/tauri.ts`, and wire it
   into the relevant store.

### Tool naming convention

- `snake_case` with `.` namespace: `script.to_video`, `timeline.add_segment`.
- Verb-first for actions: `script.parse`, `background.fetch`, `sticker.render`.
- Noun-first for queries: `system.capabilities`, `help.tool`,
  `voice.profile.list`.

### Tool response convention

Every tool returns `json!({ "status": "...", ... })`. Status values:
- `"success"` — operation completed fully
- `"warning"` — operation completed partially (e.g. no SFX found, placeholder created)
- `"rendered"` / `"assigned"` / `"built"` — domain-specific success variants
- `"error"` — never use this; return `Err(ToolError::...)` instead

When a tool creates a timeline event but cannot fill it (e.g. `sfx.assign` with
no match), return `status: "warning"`, `matched: false`, and a `message` field
explaining what the agent should do next.

---

## 5. Adding a New Tauri Command

Tauri commands are the bridge between the React frontend and the Rust backend.
They MUST be thin pass-throughs to `openscript_mcp::tools::route_tool()` — do
NOT re-implement MCP tool logic in `commands/*.rs`. The Tauri layer's only
responsibilities are:

1. In-memory project state (`AppState::projects` HashMap + `active_project` pointer)
2. Auto-save to disk after every mutation
3. Undo/redo via `UndoManager`
4. The `RENDER_IN_PROGRESS` global lock + cancel token

If you find yourself copying logic from `tools.rs` into `commands/<area>.rs`,
STOP. Refactor `tools.rs` to expose the reusable piece, then call it from both.

### Pattern

```rust
#[tauri::command]
pub async fn my_command(
    state: State<'_, AppState>,
    my_arg: String,
) -> Result<Value, String> {
    let result = openscript_mcp::tools::route_tool("my.tool", json!({"my_arg": my_arg}))
        .await
        .map_err(|e| format!("Tool failed: {}", e))?;

    // If the tool mutated a timeline, persist it
    state.with_active_project_mut(|project| {
        project.modified_at = chrono::Utc::now();
        // ... save timeline to disk ...
    });

    Ok(result)
}
```

---

## 6. Testing Protocol

### Before every commit

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cd /home/z/my-project/openscript

# 1. Build (excluding Tauri if GDK dev headers are missing)
cargo build --workspace --exclude openscript-tauri

# 2. Run all tests
cargo test --workspace --exclude openscript-tauri --lib --bins --tests

# 3. TypeScript check (frontend)
cd crates/openscript-tauri/src/frontend && npx tsc --noEmit && cd -

# 4. MCP smoke test (requires release build of mcp-server)
cargo build -p openscript-mcp --release --bin mcp-server
bash scripts/smoke_test_mcp.sh
```

All four must pass. The baseline is **248 tests**. If your change reduces this
number, you have a regression.

### Test categories

- **Unit tests** (`#[cfg(test)] mod tests`): pure-logic functions (parsers,
  validators, hashers). No I/O. Must be deterministic.
- **Integration tests** (`tests/*.rs`): exercise the MCP protocol end-to-end
  via stdin/stdout with the real `mcp-server` binary.
- **Filter graph tests** (`crates/openscript-ffmpeg/src/filter_graph.rs::tests`):
  assert on the ffmpeg filter_complex STRING produced by `FilterGraphBuilder::build()`.
  These catch regressions in the filter graph construction without running ffmpeg.
- **Smoke test** (`scripts/smoke_test_mcp.sh`): 10 key tools must be present +
  `hf.classify` returns correct recommendations for clean and useState test cases.

### What NOT to test

- Do not write tests that shell out to ffmpeg (flaky, slow, needs ffmpeg installed).
- Do not write tests that hit Pexels/GIPHY/Pixabay APIs (rate limits, needs keys).
- Do not write tests that spawn the Kokoro/Whisper Python sidecars (needs conda env).

For these, mock at the boundary: `PexelsClient` takes an API key + HTTP client;
inject a fake client in tests.

---

## 7. Git Workflow & Post-Iteration Protocol

> ### ⚠️ IRON RULE — AN ITERATION IS NOT DONE UNTIL THE COMMIT IS PUSHED
>
> **Definition of Done for every iteration:**
> 1. Code written
> 2. `cargo build --workspace --exclude openscript-tauri` passes with zero warnings
> 3. `cargo test --workspace --exclude openscript-tauri --lib --bins --tests` passes (baseline: 248 tests)
> 4. `npx tsc --noEmit` passes (if frontend changed)
> 5. `git commit` with a message following §7.2
> 6. **`git push origin main` succeeds** ← the iteration is NOT done until this prints `-> main`
> 7. `git status` shows "working tree clean" and `git log origin/main..HEAD` shows nothing
>
> If ANY of steps 2-6 fail, the iteration is NOT done. Fix it before moving on.
> If step 6 fails, **STOP. Do not start the next iteration.** See §7.5.

### 7.0 The automated post-iteration gate

Run this after every iteration. It enforces steps 2-7 above automatically:

```bash
cd /home/z/my-project/openscript && bash scripts/post-iteration.sh
```

The script:
1. Runs `cargo build` — fails on any warning
2. Runs `cargo test` — fails if any test fails or the count drops below the baseline (242)
3. Runs `npx tsc --noEmit` — fails on any TS error
4. Runs `git status` — fails if there are uncommitted changes (you forgot to commit)
5. Runs `git push origin main` — fails if push fails
6. Runs `git log origin/main..HEAD` — fails if there are unpushed commits

If the script prints `✓ POST-ITERATION GATE PASSED`, your iteration is truly done.
If it prints `✗ POST-ITERATION GATE FAILED`, you have unfinished work — fix it.

### 7.1 Commit cadence

**Push after every iteration.** An iteration = one logical unit of work
(a bug fix, a new tool, a refactor phase, a docs update). Do not accumulate
multiple unrelated changes in one commit.

- One iteration → one commit → one push. Always.
- If you find yourself writing "and also..." in a commit message, that's two
  iterations. Split it.
- If tests fail, you have NOT finished the iteration. Fix the tests, then
  commit, then push.
- If you're unsure whether to commit: commit. Small commits are cheap;
  lost work is expensive.

### 7.2 Commit message format

```
<Phase>: <one-line summary>

- <bullet describing change 1>
- <bullet describing change 2>
- <bullet describing change 3>

Tested: <what was verified>
```

Examples:
```
Phase 1: Fix 5 CRITICAL bugs
Phase 3: Fix HIGH-severity render-path bugs
Phase 7: Implement system.capabilities + help.tool meta-tools
```

### 7.3 Branch policy

- `main` is the integration branch. Push directly for small fixes.
- For large changes (>500 LoC), create a branch `feature/<name>`, open a PR,
  and require at least one review.
- Never force-push to `main`.

### 7.4 What NEVER to commit

- API keys, tokens, passwords (use env vars; the `.env` file is gitignored)
- Developer-specific paths (`/home/ishanp/...`, `/Users/yourname/...`)
- Generated files (`target/`, `node_modules/`, `*.lock` for npm)
- Binary assets that can be regenerated at runtime (procedural backgrounds,
  fetched stock music) — see §10 for the .gitignore policy
- `console.log` debug statements
- `TODO` / `FIXME` without a linked issue

### 7.5 Push-failure hard-stop protocol

**If `git push` fails for ANY reason, STOP.** Do not make another commit.
Do not write another line of code. Resolve the push first:

| Failure mode | Fix |
|-------------|-----|
| `Invalid username or token` | (1) Check `/home/z/my-project/.git-credentials` exists. (2) If missing, ask the user for a token. (3) Store it: `echo "https://ishan-parihar:TOKEN@github.com" > /home/z/my-project/.git-credentials && chmod 600 /home/z/my-project/.git-credentials`. (4) Verify: `git config --global credential.helper "store --file=/home/z/my-project/.git-credentials"`. (5) Retry push. |
| `Updates were rejected` (non-fast-forward) | `git pull --rebase origin main`, resolve conflicts, `git push origin main`. |
| `Could not resolve host` / network | Retry 3 times with `sleep 5` between. If still failing, tell the user and wait. |
| `error: file: ... cannot be pushed` (protected branch) | Tell the user; do not force-push. |

**Never have more than ONE unpushed commit on local disk.** The moment you
have an unpushed commit, the next action is either pushing it or fixing
whatever blocked the push. No new work until the push succeeds.

### 7.6 The "definition of done" checklist (print this)

```
[ ] 1. cargo build --workspace --exclude openscript-tauri  →  zero warnings
[ ] 2. cargo test --workspace --exclude openscript-tauri --lib --bins --tests  →  248+ pass
[ ] 3. (if frontend changed) npx tsc --noEmit  →  clean
[ ] 4. git add -A && git commit -m "<Phase>: <summary>"
[ ] 5. git push origin main  →  prints "main -> main"
[ ] 6. git status  →  "working tree clean"
[ ] 7. git log origin/main..HEAD  →  (empty — nothing unpushed)
```

If all 7 boxes are checked, the iteration is done. If not, you're not done.

---

## 8. Error Handling Protocol

### Rust

- **`ToolError`** (in `crates/openscript-mcp/src/error.rs`) is the canonical
  error type for all MCP tool handlers. Variants: `Ffmpeg`, `Asset`, `Timeline`,
  `NotFound`, `UnknownTool`, `Hf`, `InvalidInput`, `Io`, `Json`.
- **Never** return `Err("some string".to_string())` from a tool handler. Always
  use `ToolError::Variant(format!(...))` so the error type tells the agent what
  category of failure it was.
- **Inline error context:** when a tool wraps a lower-level call that fails
  (e.g. `render_from_timeline` returning `FfmpegError::RenderFailed(log_path)`),
  read the log file and include the last 20 lines inline in the returned error
  message. Agents cannot read separate log files — they need the context
  inline.

### TypeScript

- Every `invoke()` call must have a `.catch()` handler or be wrapped in
  `try/catch`. The error goes into the store's `error` field, which the UI
  renders as a toast.
- Never `throw` from an async function without catching it at the call site.

### Python

- Sidecars print errors to stderr as JSON: `{"error": "msg", "detail": "..."}`.
  The Rust wrapper reads stderr, parses the JSON, and converts it to a
  `ToolError`.
- Never `sys.exit(1)` without printing a JSON error first.

---

## 9. Path Resolution Protocol

OpenScript must work on any developer's machine, not just the original
author's. **Never hardcode a home directory.** Path resolution priority:

1. **Explicit env var** (e.g. `OPENSCRIPT_SFX_PATH`,
   `KOKORO_SIDECAR`, `KOKORO_PYTHON`, `PEXELS_API_KEY`)
2. **`CARGO_MANIFEST_DIR`** (compile-time workspace path; works in dev)
3. **`OPENSCRIPT_ROOT`** (deployment override)
4. **`$HOME/Videos/Assets`** (XDG-ish default for media libraries)
5. **Relative path** (last resort; only works if CWD is the repo root)

This is implemented in:
- `crates/openscript-tauri/src/state/app_state.rs::AppState::new()` (assets base)
- `crates/openscript-tts/src/kokoro_sidecar::resolve_kokoro_python()` (Kokoro Python interpreter)
- `crates/openscript-transcribe/src/transcriber.rs::find_apex_script()` (Apex wrapper)

If you add a new path resolution site, follow the same priority chain and add
a comment listing all the candidates.

---

## 10. Asset Management Protocol

### What belongs in git

- **Source code** (`.rs`, `.py`, `.ts`, `.tsx`, `.html`, `.css`, `.md`)
- **Hand-authored data** (`mcp/assets/voices.json`, `mcp/assets/svg_presets/*.svg`,
  `mcp/assets/music_index.json` for the 20 committed stock tracks)
- **Fonts** (`mcp/fonts/BebasNeue-Regular.ttf` — needed for ASS caption burning)
- **Stickers** (`mcp/assets/stickers/*.png|gif` — small, hand-curated)
- **Lockfiles** (`Cargo.lock`, `package-lock.json`)

### What does NOT belong in git

- **Generated JSON indices** (`mcp/assets/sfx_index.json`,
  `mcp/assets/music_library_index.json`) — regenerate at runtime via
  `sfx.index` / `library.build` tools. They also leak developer paths.
- **Procedural backgrounds** (`mcp/assets/backgrounds/*.mp4`) —
  `generate_procedural_background` in `tools.rs` produces them on demand.
- **Fetched stock music** (`mcp/assets/music/*.mp3`) — fetch on first run via
  Pixabay (the Rust `library_indexer.rs` module includes the local-music scan).
- **`target/`**, **`node_modules/`**, **`.env`**

The `.gitignore` enforces this. If you add a new generated artifact, add it
to `.gitignore` and document the regeneration command in the tool's
description.

---

## 11. Render Pipeline Protocol

The render pipeline has three entry points. Know which one you are using.

### `render_from_timeline` (NLE editing)

- Input: `Timeline` struct (EDL v2) + source video path
- Builds a single ffmpeg command via `FilterGraphBuilder::from_timeline()`
- Used by: `timeline.render`, `reelize.timeline`, `reelize.direct`,
  `script.to_timeline` (when caller wants a render too)
- Cancel support: `render_from_timeline_with_cancel` takes `Option<&AtomicBool>`

### `render_multilayer` (from-scratch creation)

- Input: `MultiLayerRenderSpec` (backgrounds, voiceovers, stickers, music, captions)
- Builds a multi-input ffmpeg command with concat + overlays + sidechain ducking
- Used by: `script.to_video`
- Placeholder filtering: rejects `path == "placeholder"` and empty strings

### `render` (legacy EDL v1)

- Input: `RenderConfig` (video_path, edl_path, burn_captions, srt/ass, aspect, crf, fps)
- Builds a single-input ffmpeg command via `FilterGraphBuilder::new()`
- Used by: the legacy `render` MCP tool (kept for backward compat)
- **Do not use for new code.** Use `render_from_timeline` instead.

### Placeholder b-roll filtering

Every render path MUST filter out `asset_id == "placeholder"` and empty-string
paths before they reach ffmpeg. The placeholder string crashes ffmpeg's
`movie=` filter with a cryptic "Unable to parse 'si' option value 'v'"
error. This is enforced in:
- `FilterGraphBuilder::from_timeline` (filters at the timeline level)
- `FilterGraphBuilder::with_broll` (filters at the builder level)
- `render_multilayer` (filters at the spec level)

### Loudness

`FilterGraphBuilder::normalize_lufs` (default -16.0 LUFS, EBU R128 broadcast
standard) controls the `loudnorm=I={lufs}` filter. `from_timeline` reads
`timeline.directives.mix.normalize_to_lufs` so the timeline can override it.
Never hardcode `I=-16` in a new filter string.

### Music volume

Music events carry a `gain_db` field (default -12.0 dB). Convert to linear
volume via `10f64.powf(gain_db / 20.0)`. Never hardcode `volume: 0.3` —
that ignores the timeline's specified gain.

---

## 12. Ponytail Discipline

OpenScript has a ponytail — a layer of legacy code that is no longer the
golden path but is kept for backward compatibility. The rule:

- **Legacy tools that have a golden-path replacement** (e.g. `reelize.*`
  replaced by `script.to_video`, `broll.*` replaced by `background.*`,
  `sfx.*`/`music.*` replaced by `library.*`) are kept as thin shims that
  internally call the new tool. They are NOT deleted until external callers
  have migrated.
- **Legacy tools with NO replacement** (e.g. `voice.profile.*` for voicebox
  sidecar registry) are kept as-is until a replacement is built.
- **Dead code** (unused error variants, unused structs, unused imports) is
  deleted immediately when discovered. No `#[allow(dead_code)]` without a
  comment explaining why.
- **Hand-rolled standard library** (WAV parsers, HTTP servers, base64) is
  replaced with the canonical crate (`hound`, `tower-http`, `base64`) when
  discovered. The replacement MUST fix the bugs the hand-rolled version had
  (e.g. byte-44 WAV assumption → chunk-aware `hound`).

When in doubt, run `cargo audit` and the ponytail audit script (see
`TOOL_AUDIT_REPORT.md` for the prior audit methodology).

---

## 13. Documentation Protocol

### What to update when you add/change a tool

1. `AGENT_GUIDE.md` — Tool Taxonomy section (add/modify the table row)
2. `server.rs::handle_initialize` instructions string (update tool count + trajectory)
3. `README.md` — only if the user-facing workflow changes
4. `AGENTS.md` — only if the engineering protocol changes (this file)
5. Tool `description` field in `tool_definitions()` JSON — the first sentence
   is what agents see in `tools/list`; make it count.

### What to update when you add a crate

1. `Cargo.toml` (root workspace `[workspace]` section + new crate's `Cargo.toml`)
2. This file's §1 Repository Layout
3. The crate dependency graph in §1

### What NEVER to document

- Internal implementation details that are obvious from the code
- "This is a TODO" — use the issue tracker instead
- Anything that says "as mentioned above" without adding new value

---

## 14. Security Protocol

- **No hardcoded API keys.** All API keys (Pexels, GIPHY, Pixabay) come from
  env vars: `PEXELS_API_KEY`, `GIPHY_API_KEY`, `PIXABAY_API_KEY`. The
  `system.capabilities` tool reports which are set.
- **No shell injection.** All `subprocess::Command::new` calls use
  `shell=False` and pass args as a Vec. The ffmpeg filter graph strings are
  built with `format!` and the `escape_filter_path` helper escapes single
  quotes.
- **Path traversal.** The `sanitize_input_path` helper (in `tools.rs`) rejects
  paths containing `..` segments when the path will be used to write output
  files.
- **CSP.** The Tauri `tauri.conf.json` CSP `connect-src` should be as tight
  as possible. Currently allows `http://127.0.0.1:*` — tighten to specific
  ports when stable.
- **Capabilities.** The Tauri `capabilities/default.json` should grant the
  minimum permissions needed. `shell:default` was removed in the audit pass;
  do not re-add it without scoping to specific commands.

---

## 15. Release Protocol

OpenScript does not have formal releases yet. When it does:

1. Bump version in root `Cargo.toml` and all crate `Cargo.toml`s
2. Update `CHANGELOG.md` (to be created)
3. Tag `vX.Y.Z` on the commit
4. Build release binaries for Linux, macOS, Windows
5. Update `system.capabilities` to report the version

Until then, `main` is the release. Keep it green.

---

## 16. Getting Unstuck

- **Build fails on `gdk-sys`:** You need `libgtk-3-dev` + `libwebkit2gtk-4.1-dev`
  installed. On Debian/Ubuntu: `sudo apt install libgtk-3-dev libwebkit2gtk-4.1-dev
  libayatana-appindicator3-dev librsvg2-dev`. If you cannot install these
  (no sudo), build with `--exclude openscript-tauri`.
- **MCP server hangs:** Check that stdin is being piped. The server reads
  JSON-RPC from stdin and writes to stdout. `echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' | ./target/release/mcp-server`
- **Test `test_find_conda_python_fake_env_falls_back` is flaky:** It mutates
  real env vars (process-global). Run tests single-threaded:
  `cargo test -- --test-threads=1`.
- **`hound` version conflict:** OpenScript uses `hound = "3.5"`. Earlier
  versions (0.5) do not exist on crates.io; the prior audit recommended 0.5
  but the real minimum is 3.5.
- **Frontend `tsc` fails on `RenderResult.status`:** The Rust
  `render_timeline` command returns `{output_path, file_size_bytes, status}`.
  The TS `RenderResult` interface must include all three. Check
  `lib/tauri.ts` if you added a field to the Rust response.

---

## 17. Glossary

- **EDL v1** — Edit Decision List, version 1. Legacy JSON format with
  `segments` array. Used by `edl.build` + `render` (legacy).
- **EDL v2** — Edit Decision List, version 2. Current format with `tracks`
  HashMap keyed by `TrackType`. Used by `timeline.*` tools.
- **Golden path** — The recommended trajectory for a given task. For
  from-scratch video creation, it's `script.parse` → `script.to_video`.
- **HyperFrames (HF)** — Default render engine. HTML + GSAP, rendered via
  headless Chromium. The `hf.*` tools operate on HF projects.
- **Kokoro** — Default TTS engine. 24kHz, 54 voices, ONNX-based. Runs as a
  Python sidecar (`mcp/scripts/kokoro_tts_sidecar.py`).
- **MCP** — Model Context Protocol. JSON-RPC over stdio. The server is
  `crates/openscript-mcp/src/bin/mcp-server.rs`.
- **NLE** — Non-Linear Editing. Editing existing footage (cut, caption,
  b-roll) as opposed to from-scratch creation.
- **Ponytail** — Legacy code kept for backward compatibility but no longer
  the golden path. See §12.
- **Sidecar** — A separate process spawned by Rust to do work that has no
  Rust equivalent (e.g. Python ML models). Communication is via stdin/stdout
  JSON or one-shot process spawn with file I/O.
- **Voicebox** — Optional TTS sidecar (faster-qwen3-tts) running at
  `OPENSCRIPT_TTS_URL` (default `http://127.0.0.1:17493`). Not required for
  Kokoro (the default).

---

## 18. UI Architecture Migration Plan (Stages 2 + 3)

> **Status:** Stage 1 (desktop-as-MCP-client) is COMPLETE. Stages 2 + 3 are
> documented here as a concrete implementation plan for when a Tauri-capable
> build environment (with GDK dev headers) is available. The current
> architecture is fully functional — the timeline just has a WebView ceiling.

### Why migrate beyond the WebView

The current Tauri + React frontend is the right *shell* but the wrong *media
surface*. A `<video>` tag gives play/pause/seek and nothing else — no
frame-accurate scrubbing (browsers round to keyframes), no multi-track overlay,
no GPU compositing. The timeline is HTML/CSS divs, which works for 10 segments
but jitters at 100+. Pro editors use libmpv or AVFoundation; the browser stack
is fundamentally a document renderer.

The migration keeps Tauri as the shell and splits the UI by what it's doing:

| Panel | Current | Target | Why |
|-------|---------|--------|-----|
| Timeline + video preview | React `<video>` + divs | **egui + libmpv** (native) | Frame-accurate scrubbing, real ruler, GPU compositing |
| AI command palette | React | React (keep) | Forms + text are web's strength |
| Asset browser | React | React (keep) | Grid of thumbnails — web is fine |
| Render queue | React | React (keep) | Progress bar + log — web is fine |
| Caption editor | React | React (keep) | Text editor with timestamps — web is fine |

### Stage 2: egui timeline (the hard part)

**Goal:** Replace the React `TimelineEditor` + `VideoViewport` with a native
egui panel that renders inside the Tauri window via a custom widget.

**Steps:**

1. **Add deps to `openscript-tauri/Cargo.toml`:**
   ```toml
   eframe = "0.29"        # egui framework
   egui = "0.29"
   ```

2. **Create `crates/openscript-tauri/src/egui_panel/` module:**
   - `mod.rs` — panel registration
   - `timeline.rs` — the egui timeline widget (reads `Timeline` struct directly, no JSON serialization)
   - `ruler.rs` — the time ruler (native, no PNG)
   - `playhead.rs` — the playhead (drag-to-seek, frame-accurate)

3. **Bridge egui into Tauri:** Use `tauri::WindowEvent` + a custom
   `tauri::WebviewWindowBuilder` variant that hosts an egui `eframe::App`
   alongside the React WebView. The `tauri-plugin-egui` community plugin is
   the starting point; if it doesn't fit, a raw `wgpu` surface embedded via
   `tauri::Manager::get_window` + `raw-window-handle` is the fallback.

4. **Timeline widget contract:**
   - Reads `openscript_core::timeline::Timeline` directly (no serialization)
   - Renders 6 track lanes (dialogue, voiceover, captions, broll, music, sfx)
   - Drag-to-move + drag-to-resize segment blocks
   - Right-click context menu → calls `invoke_tool("timeline.add_track_event", {...})`
   - J/K/L keyboard shuttle
   - Zoom with scroll wheel

5. **Delete the React timeline:** Once the egui timeline is stable, delete
   `components/timeline/TimelineEditor.tsx`, `TrackRow.tsx`, `SegmentBlock.tsx`,
   `Playhead.tsx`, `TimeRuler.tsx` (~400 LoC). Delete the hand-rolled media
   server in `main.rs:38-176` (~138 LoC) — egui renders to a native surface,
   no HTTP file server needed.

**Estimated effort:** 2-3 weeks for the egui widget + Tauri bridge.

### Stage 3: libmpv video preview (frame-accurate)

**Goal:** Replace the React `<video>` tag with libmpv for frame-accurate
scrubbing, multi-track audio, and hardware decode.

**Steps:**

1. **Add deps:**
   ```toml
   libmpv = "2.0"   # or mpv-rs = "0.7"
   ```

2. **System dependency:** libmpv must be installed (`libmpv-dev` on Debian,
   `mpv` via Homebrew on macOS, `mpv.exe` on Windows). Document in README.

3. **Create `crates/openscript-tauri/src/mpv_panel/` module:**
   - Renders to a native window handle that Tauri exposes as a "native view"
     alongside the egui timeline and the React WebView.
   - Exposes `seek(frame)`, `play()`, `pause()`, `set_rate(f64)` to Rust.
   - The egui timeline's playhead calls `mpv_panel.seek(frame)` on drag.

4. **Delete the React video panel:** Once libmpv is stable, delete
   `components/video/VideoViewport.tsx` + `PlaybackControls.tsx` (~130 LoC).

**Estimated effort:** 1 week for libmpv integration + the Tauri native-view
bridge (the riskiest piece — there are existing examples but it's not a
batteries-included path).

### What the final architecture looks like

```
┌──────────────────────────────────────────────────────┐
│  Tauri Window                                        │
│  ┌────────────────────────────────────────────────┐  │
│  │ Top bar: [Create from Script] [Transcribe] ... │  │
│  ├──────────────┬──────────────────┬──────────────┤  │
│  │              │                  │              │  │
│  │  egui        │  libmpv          │  React       │  │
│  │  Timeline    │  Video Preview   │  Command     │  │
│  │  (native)    │  (native)        │  Palette     │  │
│  │              │                  │  + Assets    │  │
│  │  6 tracks    │  frame-accurate  │  + Render    │  │
│  │  drag-move   │  GPU decode      │  + Captions  │  │
│  │  J/K/L       │  multi-track     │              │  │
│  │              │                  │              │  │
│  ├──────────────┴──────────────────┴──────────────┤  │
│  │ Bottom bar: [Capabilities ●] [Render Progress] │  │
│  └────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────┘
         │
         ▼
   openscript_mcp::tools::route_tool(name, args)
   (single tool surface for agents + humans)
```

### What this buys

- **Frame-accurate preview** (libmpv, not `<video>`)
- **A real timeline** (native egui widget, no div jitter)
- **~670 LoC deleted** (React timeline + video panel + media server)
- **The AI command palette stays in React** (forms + text are web's strength)
- **Cross-platform preserved** (Tauri + egui + libmpv all run on Linux/macOS/Windows)
- **Rust stays the dominant language**

### What this costs

- ~2-3 weeks for the egui timeline widget (the hard part)
- ~1 week for libmpv integration + the Tauri native-view bridge
- A Tauri plugin to host egui alongside the WebView (riskiest piece)

### When to do this

Only when the WebView ceiling is actually felt — i.e. when users complain
about timeline jitter or seek accuracy. For inspection-only use cases (the
current primary use), the React timeline is adequate. The `invoke_tool`
pass-through (Stage 1, complete) is the high-value piece that should be done
regardless; Stages 2 + 3 are optimizations.

### Bail-out ramps

- If egui-in-Tauri proves too hard, fall back to a **standalone egui binary**
  (`openscript-egui` crate) that talks to the MCP server via stdio (like an AI
  agent). This loses the integrated window but proves the egui timeline concept
  without the Tauri bridge risk.
- If libmpv is too hard, keep the React `<video>` for preview and only migrate
  the timeline to egui. The preview is "good enough" for inspection; the
  timeline is the part that genuinely suffers from divs.

---

## 19. Environment Recovery Protocol

> **This section exists because the development container was wiped between
> sessions, causing a protocol violation (3 commits were made locally without
> being pushed). This section prevents it from happening again.**

### What gets wiped

The container's filesystem can be reclaimed between sessions. Everything in
`/home/z/` except `/home/z/my-project/` may be gone. This includes:
- The openscript repo clone (if it was at `/home/z/my-project/openscript`)
- The Rust toolchain (`~/.cargo`, `~/.rustup`)
- The git remote URL with any embedded token
- Any unpushed local commits

### What survives

`/home/z/my-project/` survives container resets. It contains (or will contain
after the first recovery):
- `.git-credentials` — the GitHub token for pushing
- `.github-token` — backup copy of the token
- `openscript/` — the repo clone (re-created on recovery if missing)

### Recovery steps — run these FIRST in any new session

```bash
# 1. Verify the persistent dir exists
ls /home/z/my-project/.git-credentials 2>/dev/null || echo "NO TOKEN — ask user"

# 2. Re-clone the repo if missing
cd /home/z/my-project
[ -d openscript ] || git clone https://github.com/ishan-parihar/openscript.git

# 3. Restore the git credential helper (global config is wiped)
git config --global credential.helper "store --file=/home/z/my-project/.git-credentials"

# 4. Verify push works
cd openscript && git push origin main --dry-run 2>&1 | tail -1

# 5. If push fails, STOP and ask the user for a token before making any commits.

# 6. Run setup.sh to restore the toolchain (Rust, Python deps, Kokoro model,
#    build artifacts). Idempotent — skips what's already done.
#    See setup.sh --help for options.
bash setup.sh
```

### Iron rule reinforcement — push failure = hard stop

**If `git push` fails for ANY reason, STOP.** Do not make another commit.
Do not write another line of code. Resolve the push first:

1. If auth failure → ask the user for a token, store it in
   `/home/z/my-project/.git-credentials`, retry.
2. If non-fast-forward → `git pull --rebase origin main`, resolve conflicts,
   retry.
3. If network failure → retry up to 3 times with 5s sleep. If still failing,
   tell the user and wait.

**Never have more than ONE unpushed commit on local disk.** The moment you
have an unpushed commit, the next action is either pushing it or fixing
whatever blocked the push. No new work until the push succeeds.

---

## 20. R&D Protocol — Refactor/Upgrade Gate

> **Every refactor or upgrade is an iteration. It MUST end with verification,
> lint, commit, and push — no exceptions.**

### What counts as a refactor/upgrade

Any change that modifies code structure, dependencies, tool schemas, render
paths, or documentation — even if no new features are added. Examples:
- Renaming a function, moving a module, extracting a helper
- Updating a dependency version
- Changing a tool's `inputSchema` or `description`
- Modifying the render pipeline or filter graph
- Updating AGENTS.md, AGENT_GUIDE.md, or README.md

### The gate (mandatory after every refactor/upgrade)

```
1. cargo build --workspace --exclude openscript-tauri   →  zero warnings
2. cargo test --workspace --exclude openscript-tauri --lib --bins --tests  →  pass
3. npx tsc --noEmit  (if frontend changed)              →  clean
4. python3 scripts/workspace-lint/workspace_lint.py --root .  →  zero errors
5. git add -A && git commit -m "<Phase>: <summary>"
6. git push github main
7. git status  →  "working tree clean"
```

**Steps 1-4 are verification. Steps 5-7 are commit/push.** All 7 must pass.
If any verification step fails, fix it before committing. If push fails,
STOP — see §7.5.

### Why workspace lint matters

The workspace-lint validator (`workspace-lint.yaml` + `scripts/workspace-lint/workspace_lint.py`)
catches:
- **Scratch files at root** — `.timeline.json`, `.edl.json`, `.ass`, `.wav`,
  `output.mp4` that should be in `artifacts/`
- **Forbidden files** — `.log`, `.tmp`, `.pyc`, render artifacts
- **Oversized files** — binaries or assets that exceed size limits
- **Misplaced assets** — files in the wrong directory

Without the lint gate, scratch files accumulate at root and eventually get
committed by accident. The lint run takes <1s and prevents this.

### Quick reference

```bash
# Full R&D gate (all steps):
cargo build --workspace --exclude openscript-tauri && \
cargo test --workspace --exclude openscript-tauri --lib --bins --tests && \
python3 scripts/workspace-lint/workspace_lint.py --root . && \
git add -A && git commit -m "Phase X: <summary>" && \
git push github main && \
git status

# Lint only (fast check):
python3 scripts/workspace-lint/workspace_lint.py --root .
```
