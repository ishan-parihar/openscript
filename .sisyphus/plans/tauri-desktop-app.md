# OpenScript Tauri Desktop App Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Descript-like desktop video editor using Tauri (Rust backend + React frontend), with text-based editing, multi-track timeline, asset libraries, and AI integration.

**Architecture:** New `openscript-tauri` crate wraps existing Rust crates as Tauri commands. React/TypeScript frontend runs in Tauri's Webview with Zustand state management. All 43 MCP tools become `#[tauri::command]` functions with zero-copy access to existing backend logic.

**Tech Stack:** Tauri 2.x, Rust (existing 8 crates), React 18, TypeScript, Tailwind CSS, Zustand, shadcn/ui, wavesurfer.js, TipTap

---

## File Structure Overview

### New Rust Crate: `openscript-tauri`
```
crates/openscript-tauri/
├── Cargo.toml
├── tauri.conf.json
├── src/
│   ├── main.rs              # Tauri app entry, command registry
│   ├── commands/
│   │   ├── mod.rs           # Barrel exports
│   │   ├── project.rs       # timeline.build/load/save/create
│   │   ├── transcript.rs    # transcribe, srt.read, srt.prepare
│   │   ├── timeline.rs      # timeline.add_segment, split, diff, preview
│   │   ├── render.rs        # timeline.render, verify.*
│   │   ├── assets.rs        # broll.*, music.*, sfx.*
│   │   ├── voice.rs         # voice.profile.*, tts.*, voiceover.*
│   │   ├── motion.rs        # motion.*
│   │   └── system.rs        # system.capabilities, reelize.*
│   ├── state/
│   │   ├── mod.rs
│   │   ├── app_state.rs     # Global AppState (projects, undo stack)
│   │   └── undo.rs          # Undo/redo manager
│   └── utils/
│       ├── mod.rs
│       └── file_watcher.rs  # File change notifications
```

### Frontend (inside Tauri's src/ directory)
```
crates/openscript-tauri/src/
├── frontend/
│   ├── package.json
│   ├── tsconfig.json
│   ├── vite.config.ts
│   ├── tailwind.config.ts
│   ├── index.html
│   └── src/
│       ├── main.tsx
│       ├── App.tsx
│       ├── styles/
│       │   └── globals.css
│       ├── lib/
│       │   ├── tauri.ts        # invoke() wrappers for all commands
│       │   ├── utils.ts        # cn() helper, formatters
│       │   └── time.ts         # ms-to-frames, duration formatting
│       ├── store/
│       │   ├── project.ts      # ProjectState (Zustand)
│       │   ├── editor.ts       # EditorState (selection, playback, zoom)
│       │   ├── transcript.ts   # TranscriptState (segments, editing)
│       │   ├── assets.ts       # AssetState (broll, music, sfx)
│       │   └── ai.ts           # AIState (chat, suggestions)
│       ├── components/
│       │   ├── layout/
│       │   │   ├── AppLayout.tsx        # Main 3-panel layout
│       │   │   ├── TopBar.tsx           # Project name, menu, render button
│       │   │   └── StatusBar.tsx        # Playback position, zoom, status
│       │   ├── transcript/
│       │   │   ├── TranscriptEditor.tsx # TipTap-based text editor
│       │   │   ├── TranscriptSegment.tsx
│       │   │   └── WordToken.tsx        # Individual word (clickable)
│       │   ├── timeline/
│       │   │   ├── TimelineEditor.tsx   # Main timeline canvas
│       │   │   ├── TrackRow.tsx         # Single track row
│       │   │   ├── SegmentBlock.tsx     # Draggable segment
│       │   │   ├── Playhead.tsx         # Red vertical line
│       │   │   ├── TimeRuler.tsx        # Top time axis
│       │   │   └── WaveformDisplay.tsx  # wavesurfer.js wrapper
│       │   ├── assets/
│       │   │   ├── AssetBrowser.tsx     # Tabbed: B-Roll / Music / SFX
│       │   │   ├── BrollGrid.tsx        # Video preview cards
│       │   │   ├── MusicList.tsx        # Mood/energy filtered list
│       │   │   └── SFXList.tsx          # Role-based search
│       │   ├── ai/
│       │   │   ├── AIAssistant.tsx      # Chat panel
│       │   │   ├── AISuggestions.tsx    # Auto-suggestion chips
│       │   │   └── ReelizeButton.tsx    # One-call pipeline trigger
│       │   └── shared/
│       │       ├── Button.tsx
│       │       ├── Dialog.tsx
│       │       ├── Input.tsx
│       │       ├── Badge.tsx
│       │       └── Progress.tsx
│       └── hooks/
│           ├── usePlayback.ts
│           ├── useKeyboardShortcuts.ts
│           └── useFileDrop.ts
```

### Modified Existing Files
```
Cargo.toml (root workspace)          # Add openscript-tauri to members
crates/openscript-core/src/types.rs  # Add Speaker detection types
crates/openscript-core/src/srt/mod.rs # Add filler word detection
crates/openscript-core/src/timeline/schema.rs # Add split_segment, undo support
```

---

## Phase 1: Foundation — Tauri Scaffolding + Core Loop

### Task 1.1: Create Tauri Crate Scaffold

**Files:**
- Create: `crates/openscript-tauri/Cargo.toml`
- Create: `crates/openscript-tauri/tauri.conf.json`
- Create: `crates/openscript-tauri/src/main.rs`
- Create: `crates/openscript-tauri/capabilities/default.json`
- Create: `crates/openscript-tauri/icons/` (placeholder)
- Modify: `Cargo.toml` (root workspace)

- [ ] **Step 1: Add openscript-tauri to workspace members**

```toml
# Cargo.toml (root) — add to [workspace] members
members = [
  "crates/openscript-core",
  "crates/openscript-mcp",
  "crates/openscript-ffmpeg",
  "crates/openscript-transcribe",
  "crates/openscript-tts",
  "crates/openscript-assets",
  "crates/openscript-ui",
  "crates/openscript-cli",
  "crates/openscript-tauri",
]
```

- [ ] **Step 2: Create Tauri crate Cargo.toml**

```toml
# crates/openscript-tauri/Cargo.toml
[package]
name = "openscript-tauri"
version = "0.1.0"
edition = "2021"
description = "OpenScript Desktop — AI-directed video editor"

[dependencies]
tauri = { version = "2", features = ["devtools"] }
tauri-plugin-dialog = "2"
tauri-plugin-fs = "2"
tauri-plugin-shell = "2"
serde = { workspace = true }
serde_json = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true, features = ["env-filter"] }
thiserror = { workspace = true }

openscript-core = { path = "../openscript-core" }
openscript-ffmpeg = { path = "../openscript-ffmpeg" }
openscript-transcribe = { path = "../openscript-transcribe" }
openscript-tts = { path = "../openscript-tts" }
openscript-assets = { path = "../openscript-assets" }

[build-dependencies]
tauri-build = { version = "2" }
```

- [ ] **Step 3: Create Tauri config**

```json
// crates/openscript-tauri/tauri.conf.json
{
  "productName": "OpenScript",
  "version": "0.1.0",
  "identifier": "com.openscript.editor",
  "build": {
    "beforeDevCommand": "cd src/frontend && npm run dev",
    "devUrl": "http://localhost:1420",
    "frontendDist": "../src/frontend/dist"
  },
  "app": {
    "withGlobalTauri": true,
    "windows": [
      {
        "title": "OpenScript",
        "width": 1440,
        "height": 900,
        "minWidth": 1024,
        "minHeight": 700,
        "resizable": true
      }
    ],
    "security": {
      "csp": "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob: file:; media-src 'self' file: blob:; connect-src 'self' http://localhost:* http://127.0.0.1:*"
    }
  },
  "bundle": {
    "active": true,
    "targets": "all",
    "icon": [
      "icons/32x32.png",
      "icons/128x128.png",
      "icons/128x128@2x.png",
      "icons/icon.icns",
      "icons/icon.ico"
    ]
  }
}
```

- [ ] **Step 4: Create build.rs**

```rust
// crates/openscript-tauri/build.rs
fn main() {
    tauri_build::build()
}
```

- [ ] **Step 5: Create main.rs — bare Tauri app**

```rust
// crates/openscript-tauri/src/main.rs
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "openscript_tauri=debug,tauri=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .setup(|_app| {
            tracing::info!("OpenScript Tauri app initialized");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

- [ ] **Step 6: Create default capability**

```json
// crates/openscript-tauri/capabilities/default.json
{
  "identifier": "default",
  "description": "Default capabilities for OpenScript",
  "windows": ["main"],
  "permissions": [
    "core:default",
    "dialog:default",
    "fs:default",
    "fs:allow-read-file",
    "fs:allow-write-file",
    "fs:allow-read-dir",
    "fs:scope-appconfig-recursive",
    "shell:default"
  ]
}
```

- [ ] **Step 7: Create placeholder icons directory**

```bash
mkdir -p crates/openscript-tauri/icons
# Create a minimal 32x32 PNG placeholder (any valid PNG)
# Use ImageMagick or just create a small valid PNG
python3 -c "
import struct, zlib
def create_png(w, h, color=(100,100,100)):
    def chunk(ctype, data):
        c = ctype + data
        return struct.pack('>I', len(data)) + c + struct.pack('>I', zlib.crc32(c) & 0xffffffff)
    header = struct.pack('8B', 137, 80, 78, 71, 13, 10, 26, 10)
    ihdr = chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0))
    raw = b''
    for y in range(h):
        raw += b'\x00'
        for x in range(w):
            raw += struct.pack('3B', *color)
    idat = chunk(b'IDAT', zlib.compress(raw))
    iend = chunk(b'IEND', b'')
    return header + ihdr + idat + iend
with open('crates/openscript-tauri/icons/32x32.png', 'wb') as f:
    f.write(create_png(32, 32))
"
```

- [ ] **Step 8: Verify Tauri build compiles**

```bash
cd /home/ishanp/Documents/GitHub/openscript
cargo check -p openscript-tauri
```

Expected: Compiles successfully (frontend will fail — that's fine for now).

- [ ] **Step 9: Commit**

```bash
git add crates/openscript-tauri/ Cargo.toml
git commit -m "feat: scaffold openscript-tauri crate with Tauri 2.x"
```

---

### Task 1.2: Frontend Scaffold — React + Vite + Tailwind

**Files:**
- Create: `crates/openscript-tauri/src/frontend/package.json`
- Create: `crates/openscript-tauri/src/frontend/vite.config.ts`
- Create: `crates/openscript-tauri/src/frontend/tsconfig.json`
- Create: `crates/openscript-tauri/src/frontend/tailwind.config.ts`
- Create: `crates/openscript-tauri/src/frontend/postcss.config.js`
- Create: `crates/openscript-tauri/src/frontend/index.html`
- Create: `crates/openscript-tauri/src/frontend/src/main.tsx`
- Create: `crates/openscript-tauri/src/frontend/src/App.tsx`
- Create: `crates/openscript-tauri/src/frontend/src/styles/globals.css`

- [ ] **Step 1: Create package.json**

```json
{
  "name": "openscript-frontend",
  "private": true,
  "version": "0.1.0",
  "type": "module",
  "scripts": {
    "dev": "vite",
    "build": "tsc -b && vite build",
    "preview": "vite preview",
    "lint": "eslint ."
  },
  "dependencies": {
    "react": "^18.3.1",
    "react-dom": "^18.3.1",
    "@tauri-apps/api": "^2.0.0",
    "@tauri-apps/plugin-dialog": "^2.0.0",
    "@tauri-apps/plugin-fs": "^2.0.0",
    "zustand": "^5.0.0",
    "@tiptap/react": "^2.11.0",
    "@tiptap/starter-kit": "^2.11.0",
    "wavesurfer.js": "^7.8.0",
    "clsx": "^2.1.1",
    "tailwind-merge": "^2.5.0",
    "lucide-react": "^0.460.0"
  },
  "devDependencies": {
    "@types/react": "^18.3.12",
    "@types/react-dom": "^18.3.1",
    "@vitejs/plugin-react": "^4.3.4",
    "typescript": "~5.6.0",
    "vite": "^6.0.0",
    "tailwindcss": "^3.4.15",
    "postcss": "^8.4.49",
    "autoprefixer": "^10.4.20"
  }
}
```

- [ ] **Step 2: Create Vite config**

```typescript
// crates/openscript-tauri/src/frontend/vite.config.ts
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
});
```

- [ ] **Step 3: Create tsconfig.json**

```json
{
  "compilerOptions": {
    "target": "ES2020",
    "useDefineForClassFields": true,
    "lib": ["ES2020", "DOM", "DOM.Iterable"],
    "module": "ESNext",
    "skipLibCheck": true,
    "moduleResolution": "bundler",
    "allowImportingTsExtensions": true,
    "isolatedModules": true,
    "moduleDetection": "force",
    "noEmit": true,
    "jsx": "react-jsx",
    "strict": true,
    "noUnusedLocals": true,
    "noUnusedParameters": true,
    "noFallthroughCasesInSwitch": true,
    "baseUrl": ".",
    "paths": {
      "@/*": ["src/*"]
    }
  },
  "include": ["src"]
}
```

- [ ] **Step 4: Create Tailwind + PostCSS configs**

```typescript
// crates/openscript-tauri/src/frontend/tailwind.config.ts
import type { Config } from "tailwindcss";

export default {
  content: ["./index.html", "./src/**/*.{js,ts,jsx,tsx}"],
  theme: {
    extend: {
      colors: {
        border: "hsl(240 5.9% 90%)",
        input: "hsl(240 5.9% 90%)",
        ring: "hsl(240 5.9% 10%)",
        background: "hsl(0 0% 100%)",
        foreground: "hsl(240 10% 3.9%)",
        primary: { DEFAULT: "hsl(240 5.9% 10%)", foreground: "hsl(0 0% 98%)" },
        secondary: { DEFAULT: "hsl(240 4.8% 95.9%)", foreground: "hsl(240 5.9% 10%)" },
        muted: { DEFAULT: "hsl(240 4.8% 95.9%)", foreground: "hsl(240 3.8% 46.1%)" },
        accent: { DEFAULT: "hsl(240 4.8% 95.9%)", foreground: "hsl(240 5.9% 10%)" },
        destructive: { DEFAULT: "hsl(0 84.2% 60.2%)", foreground: "hsl(0 0% 98%)" },
      },
    },
  },
  plugins: [],
} satisfies Config;
```

```javascript
// crates/openscript-tauri/src/frontend/postcss.config.js
export default {
  plugins: {
    tailwindcss: {},
    autoprefixer: {},
  },
};
```

- [ ] **Step 5: Create index.html**

```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>OpenScript</title>
  </head>
  <body>
    <div id="root"></div>
    <script type="module" src="/src/main.tsx"></script>
  </body>
</html>
```

- [ ] **Step 6: Create entry point and App**

```typescript
// crates/openscript-tauri/src/frontend/src/main.tsx
import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles/globals.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
```

```typescript
// crates/openscript-tauri/src/frontend/src/App.tsx
function App() {
  return (
    <div className="flex h-screen w-screen items-center justify-center bg-background text-foreground">
      <div className="text-center">
        <h1 className="text-2xl font-bold">OpenScript</h1>
        <p className="mt-2 text-muted-foreground">
          AI-directed video editor — Tauri frontend loading...
        </p>
        <p className="mt-4 text-sm text-green-600">
          Tauri connected ✓
        </p>
      </div>
    </div>
  );
}

export default App;
```

```css
/* crates/openscript-tauri/src/frontend/src/styles/globals.css */
@tailwind base;
@tailwind components;
@tailwind utilities;

@layer base {
  * {
    @apply border-border;
  }
  body {
    @apply bg-background text-foreground;
    font-feature-settings: "rlig" 1, "calt" 1;
  }
}
```

- [ ] **Step 7: Install dependencies and verify**

```bash
cd crates/openscript-tauri/src/frontend
npm install
npm run build
```

Expected: Clean build, no errors.

- [ ] **Step 8: Commit**

```bash
git add crates/openscript-tauri/src/frontend/
git commit -m "feat: scaffold React+Vite+Tailwind frontend"
```

---

### Task 1.3: AppState + Undo/Redo Manager

**Files:**
- Create: `crates/openscript-tauri/src/state/mod.rs`
- Create: `crates/openscript-tauri/src/state/app_state.rs`
- Create: `crates/openscript-tauri/src/state/undo.rs`
- Modify: `crates/openscript-tauri/src/main.rs`

- [ ] **Step 1: Create Undo/Redo manager**

```rust
// crates/openscript-tauri/src/state/undo.rs
use serde_json::Value;
use std::collections::VecDeque;

const MAX_UNDO_DEPTH: usize = 50;

/// A single undoable operation.
#[derive(Debug, Clone)]
pub struct UndoEntry {
    /// Human-readable description of the operation
    pub description: String,
    /// The timeline state BEFORE the operation (for undo)
    pub before: Value,
    /// The timeline state AFTER the operation (for redo)
    pub after: Value,
}

/// Stack-based undo/redo manager for timeline operations.
pub struct UndoManager {
    undo_stack: VecDeque<UndoEntry>,
    redo_stack: VecDeque<UndoEntry>,
    max_depth: usize,
}

impl UndoManager {
    pub fn new() -> Self {
        Self {
            undo_stack: VecDeque::with_capacity(MAX_UNDO_DEPTH),
            redo_stack: VecDeque::with_capacity(MAX_UNDO_DEPTH),
            max_depth: MAX_UNDO_DEPTH,
        }
    }

    /// Record an operation. Call this AFTER the operation succeeds.
    pub fn record(&mut self, description: String, before: Value, after: Value) {
        let entry = UndoEntry { description, before, after };
        self.undo_stack.push_back(entry);
        // Clear redo stack on new operation
        self.redo_stack.clear();
        // Enforce max depth
        if self.undo_stack.len() > self.max_depth {
            self.undo_stack.pop_front();
        }
    }

    /// Undo the last operation. Returns the state to restore to, or None if nothing to undo.
    pub fn undo(&mut self) -> Option<(String, Value)> {
        self.undo_stack.pop_back().map(|entry| {
            self.redo_stack.push_back(UndoEntry {
                description: entry.description.clone(),
                before: entry.before.clone(),
                after: entry.after.clone(),
            });
            (entry.description, entry.before)
        })
    }

    /// Redo the last undone operation. Returns the state to restore to, or None if nothing to redo.
    pub fn redo(&mut self) -> Option<(String, Value)> {
        self.redo_stack.pop_back().map(|entry| {
            self.undo_stack.push_back(UndoEntry {
                description: entry.description.clone(),
                before: entry.before.clone(),
                after: entry.after.clone(),
            });
            (entry.description, entry.after)
        })
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn clear(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
    }

    pub fn undo_count(&self) -> usize {
        self.undo_stack.len()
    }

    pub fn redo_count(&self) -> usize {
        self.redo_stack.len()
    }
}

impl Default for UndoManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn val(n: i32) -> Value {
        Value::Number(n.into())
    }

    #[test]
    fn test_undo_redo_basic() {
        let mut mgr = UndoManager::new();

        // Simulate: timeline goes from 1 → 2 → 3
        mgr.record("add segment A".into(), val(1), val(2));
        mgr.record("add segment B".into(), val(2), val(3));

        assert!(mgr.can_undo());
        assert!(!mgr.can_redo());

        // Undo: 3 → 2
        let (desc, state) = mgr.undo().unwrap();
        assert_eq!(desc, "add segment B");
        assert_eq!(state, val(2));

        assert!(mgr.can_undo());
        assert!(mgr.can_redo());

        // Redo: 2 → 3
        let (desc, state) = mgr.redo().unwrap();
        assert_eq!(desc, "add segment B");
        assert_eq!(state, val(3));
    }

    #[test]
    fn test_new_operation_clears_redo() {
        let mut mgr = UndoManager::new();
        mgr.record("op1".into(), val(1), val(2));
        mgr.record("op2".into(), val(2), val(3));
        mgr.undo(); // Back to 2
        assert!(mgr.can_redo());

        // New operation clears redo stack
        mgr.record("op3".into(), val(2), val(4));
        assert!(!mgr.can_redo());
    }

    #[test]
    fn test_max_depth() {
        let mut mgr = UndoManager::new();
        for i in 0..60 {
            mgr.record(format!("op {}", i), val(i), val(i + 1));
        }
        assert_eq!(mgr.undo_count(), 50); // Capped at MAX_UNDO_DEPTH
    }
}
```

- [ ] **Step 2: Create AppState**

```rust
// crates/openscript-tauri/src/state/app_state.rs
use openscript_core::timeline::schema::Timeline;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use super::undo::UndoManager;

/// A project in the OpenScript editor.
#[derive(Debug)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub source_video_path: String,
    pub timeline_path: String,
    pub transcript_path: Option<String>,
    pub timeline: Timeline,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub modified_at: chrono::DateTime<chrono::Utc>,
}

impl Project {
    pub fn new(id: String, name: String, source_video: String, timeline_path: String, timeline: Timeline) -> Self {
        let now = chrono::Utc::now();
        Self {
            id,
            name,
            source_video_path: source_video,
            timeline_path,
            transcript_path: None,
            timeline,
            created_at: now,
            modified_at: now,
        }
    }
}

/// Global application state managed by Tauri.
pub struct AppState {
    /// Open projects (keyed by project ID)
    pub projects: Arc<RwLock<HashMap<String, Project>>>,
    /// Currently active project ID
    pub active_project: Arc<RwLock<Option<String>>>,
    /// Undo/redo manager for the active project
    pub undo_manager: Arc<RwLock<UndoManager>>,
    /// Path to the assets directory (SFX, music)
    pub assets_base_path: PathBuf,
    /// TTS service URL
    pub tts_url: String,
    /// Pexels API key (if configured)
    pub pexels_api_key: Option<String>,
}

impl AppState {
    pub fn new() -> Self {
        let assets_base = std::env::var("OPENSCRIPT_SFX_PATH")
            .ok()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/home/ishanp/Videos/Assets"));

        let tts_url = std::env::var("OPENSCRIPT_TTS_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:17493".to_string());

        let pexels_key = std::env::var("PEXELS_API_KEY").ok();

        Self {
            projects: Arc::new(RwLock::new(HashMap::new())),
            active_project: Arc::new(RwLock::new(None)),
            undo_manager: Arc::new(RwLock::new(UndoManager::new())),
            assets_base_path: assets_base,
            tts_url,
            pexels_api_key: pexels_key,
        }
    }

    /// Get the active project (read-only).
    pub fn active_project(&self) -> Option<Project> {
        let guard = self.active_project.read().ok()?;
        let id = guard.as_ref()?;
        let projects = self.projects.read().ok()?;
        projects.get(id).cloned()
    }

    /// Get the active project (mutable).
    pub fn with_active_project_mut<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&mut Project) -> R,
    {
        let guard = self.active_project.read().ok()?;
        let id = guard.as_ref()?.clone();
        drop(guard);
        let mut projects = self.projects.write().ok()?;
        let project = projects.get_mut(&id)?;
        project.modified_at = chrono::Utc::now();
        Some(f(project))
    }

    /// Get the active project's timeline (mutable).
    pub fn with_active_timeline_mut<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&mut Timeline) -> R,
    {
        self.with_active_project_mut(|project| f(&mut project.timeline))
    }

    /// Get the current timeline as JSON (for undo snapshots).
    pub fn timeline_snapshot(&self) -> Option<Value> {
        self.with_active_project(|project| {
            serde_json::to_value(&project.timeline).ok()
        })?
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
```

Note: This requires `chrono` as a dependency. Add it:

```toml
# crates/openscript-tauri/Cargo.toml — add to [dependencies]
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1", features = ["v4"] }
```

- [ ] **Step 3: Create state barrel module**

```rust
// crates/openscript-tauri/src/state/mod.rs
pub mod app_state;
pub mod undo;

pub use app_state::AppState;
pub use undo::UndoManager;
```

- [ ] **Step 4: Wire AppState into main.rs**

```rust
// crates/openscript-tauri/src/main.rs — update
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod state;

use state::AppState;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "openscript_tauri=debug,tauri=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let app_state = AppState::new();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .manage(app_state)
        .setup(|_app| {
            tracing::info!("OpenScript Tauri app initialized");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Commands will be registered here in subsequent tasks
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

- [ ] **Step 5: Create empty commands barrel**

```rust
// crates/openscript-tauri/src/commands/mod.rs
// Command modules — populated as tasks progress
pub mod project;
pub mod transcript;
pub mod timeline;
pub mod render;
pub mod assets;
pub mod voice;
pub mod motion;
pub mod system;
```

Create stub files for each (will be populated in later tasks):
```bash
for f in project transcript timeline render assets voice motion system; do
  echo "// ${f} commands" > crates/openscript-tauri/src/commands/${f}.rs
done
```

- [ ] **Step 6: Run tests**

```bash
cd /home/ishanp/Documents/GitHub/openscript
cargo test -p openscript-tauri undo
```

Expected: 3 tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/openscript-tauri/src/state/ crates/openscript-tauri/src/main.rs crates/openscript-tauri/src/commands/ crates/openscript-tauri/Cargo.toml
git commit -m "feat: AppState with undo/redo manager, wire into Tauri"
```

---

### Task 1.4: Wire 5 Core Tauri Commands

**Files:**
- Modify: `crates/openscript-tauri/src/commands/project.rs`
- Modify: `crates/openscript-tauri/src/commands/timeline.rs`
- Modify: `crates/openscript-tauri/src/commands/mod.rs`
- Modify: `crates/openscript-tauri/src/main.rs`

- [ ] **Step 1: Implement project commands**

```rust
// crates/openscript-tauri/src/commands/project.rs
use openscript_core::timeline::schema::Timeline;
use serde_json::{json, Value};
use std::path::PathBuf;
use tauri::State;
use uuid::Uuid;

use crate::state::AppState;

/// Create a new project from a source video.
/// Returns project ID and initial timeline path.
#[tauri::command]
pub async fn create_project(
    state: State<'_, AppState>,
    source_video: String,
) -> Result<Value, String> {
    let video_path = PathBuf::from(&source_video);
    if !video_path.exists() {
        return Err(format!("Source video not found: {}", source_video));
    }

    let project_id = Uuid::new_v4().to_string();
    let project_name = video_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Untitled")
        .to_string();

    // Create timeline using openscript_core
    let timeline_dir = PathBuf::from(format!(".openscript/projects/{}", project_id));
    std::fs::create_dir_all(&timeline_dir).map_err(|e| format!("Failed to create project dir: {}", e))?;

    let timeline_path = timeline_dir.join("timeline.json");
    let timeline = Timeline::new(
        source_video.clone(),
        1080,
        1920,
        30,
    );

    // Save timeline
    let timeline_json = serde_json::to_string_pretty(&timeline)
        .map_err(|e| format!("Failed to serialize timeline: {}", e))?;
    std::fs::write(&timeline_path, &timeline_json)
        .map_err(|e| format!("Failed to write timeline: {}", e))?;

    // Register in AppState
    let mut projects = state.projects.write().map_err(|_| "Lock poisoned")?;
    let project = crate::state::app_state::Project::new(
        project_id.clone(),
        project_name,
        source_video,
        timeline_path.to_string_lossy().to_string(),
        timeline,
    );
    projects.insert(project_id.clone(), project);

    *state.active_project.write().map_err(|_| "Lock poisoned")? = Some(project_id.clone());

    Ok(json!({
        "project_id": project_id,
        "name": project_name,
        "timeline_path": timeline_path.to_string_lossy(),
    }))
}

/// Load an existing project by ID.
#[tauri::command]
pub async fn load_project(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Value, String> {
    // Try in-memory first
    {
        let projects = state.projects.read().map_err(|_| "Lock poisoned")?;
        if let Some(project) = projects.get(&project_id) {
            *state.active_project.write().map_err(|_| "Lock poisoned")? = Some(project_id);
            return Ok(json!({
                "project_id": project.id,
                "name": project.name,
                "source_video": project.source_video_path,
                "timeline_path": project.timeline_path,
                "timeline": serde_json::to_value(&project.timeline)?,
            }));
        }
    }

    // Try loading from disk
    let timeline_path = PathBuf::from(format!(".openscript/projects/{}/timeline.json", project_id));
    if !timeline_path.exists() {
        return Err(format!("Project not found: {}", project_id));
    }

    let content = std::fs::read_to_string(&timeline_path)
        .map_err(|e| format!("Failed to read timeline: {}", e))?;
    let timeline: Timeline = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse timeline: {}", e))?;

    let project = crate::state::app_state::Project::new(
        project_id.clone(),
        "Loaded Project".to_string(),
        timeline.source.clone(),
        timeline_path.to_string_lossy().to_string(),
        timeline,
    );

    let mut projects = state.projects.write().map_err(|_| "Lock poisoned")?;
    projects.insert(project_id.clone(), project);
    *state.active_project.write().map_err(|_| "Lock poisoned")? = Some(project_id);

    Ok(json!({ "project_id": project_id, "loaded": true }))
}

/// List all open projects.
#[tauri::command]
pub async fn list_projects(state: State<'_, AppState>) -> Result<Value, String> {
    let projects = state.projects.read().map_err(|_| "Lock poisoned")?;
    let active = state.active_project.read().map_err(|_| "Lock poisoned")?;

    let list: Vec<Value> = projects.values().map(|p| {
        json!({
            "id": p.id,
            "name": p.name,
            "source_video": p.source_video_path,
            "active": active.as_ref() == Some(&p.id),
        })
    }).collect();

    Ok(json!(list))
}

/// Save the active project's timeline to disk.
#[tauri::command]
pub async fn save_project(state: State<'_, AppState>) -> Result<Value, String> {
    let timeline_path = state.with_active_project(|p| p.timeline_path.clone())
        .ok_or("No active project")?;

    let timeline_json = state.with_active_project(|p| {
        serde_json::to_string_pretty(&p.timeline)
    }).ok_or("Failed to serialize timeline")??;

    std::fs::write(&timeline_path, &timeline_json)
        .map_err(|e| format!("Failed to save: {}", e))?;

    Ok(json!({ "saved": true, "path": timeline_path }))
}
```

- [ ] **Step 2: Implement timeline commands**

```rust
// crates/openscript-tauri/src/commands/timeline.rs
use openscript_core::timeline::schema::Timeline;
use serde_json::{json, Value};
use tauri::State;

use crate::state::AppState;

/// Add a segment to the active project's timeline.
#[tauri::command]
pub async fn add_segment(
    state: State<'_, AppState>,
    start: f64,
    end: f64,
    caption: String,
    semantic_role: Option<String>,
) -> Result<Value, String> {
    let snapshot_before = state.timeline_snapshot()
        .ok_or("No active project")?;

    let segment_id = state.with_active_timeline_mut(|timeline| {
        // Use openscript_core's Timeline API directly
        let id = format!("seg_{:03}", timeline.segments.len() + 1);
        let segment = openscript_core::timeline::schema::Segment {
            id: id.clone(),
            source_start_ms: (start * 1000.0) as u64,
            source_end_ms: (end * 1000.0) as u64,
            caption: caption.clone(),
            semantic_role: semantic_role.clone(),
            ..Default::default()
        };
        timeline.segments.push(segment);
        id
    }).ok_or("Failed to add segment")?;

    let snapshot_after = state.timeline_snapshot()
        .ok_or("No active project")?;

    // Record for undo
    state.undo_manager.write().map_err(|_| "Lock poisoned")?
        .record(format!("Add segment: {}", caption), snapshot_before, snapshot_after);

    // Auto-save
    let _ = save_project_inner(&state);

    Ok(json!({ "segment_id": segment_id }))
}

/// Get the active timeline as JSON.
#[tauri::command]
pub async fn get_timeline(state: State<'_, AppState>) -> Result<Value, String> {
    state.with_active_project(|project| {
        json!({
            "project_id": project.id,
            "name": project.name,
            "source_video": project.source_video_path,
            "timeline": serde_json::to_value(&project.timeline),
            "segment_count": project.timeline.segments.len(),
        })
    }).ok_or("No active project")
}

/// Get timeline preview summary.
#[tauri::command]
pub async fn timeline_preview(state: State<'_, AppState>) -> Result<Value, String> {
    state.with_active_project(|project| {
        let total_duration_ms = project.timeline.segments.iter()
            .map(|s| s.source_end_ms.saturating_sub(s.source_start_ms))
            .sum::<u64>();

        let track_counts: Value = serde_json::to_value(&project.timeline.tracks)
            .unwrap_or_default();

        json!({
            "total_duration_ms": total_duration_ms,
            "segment_count": project.timeline.segments.len(),
            "tracks": track_counts,
            "render_ready": project.timeline.segments.len() > 0,
        })
    }).ok_or("No active project")
}

/// Undo the last operation.
#[tauri::command]
pub async fn undo(state: State<'_, AppState>) -> Result<Value, String> {
    let (desc, snapshot) = state.undo_manager.write()
        .map_err(|_| "Lock poisoned")?
        .undo()
        .ok_or("Nothing to undo")?;

    // Restore timeline from snapshot
    state.with_active_project_mut(|project| {
        project.timeline = serde_json::from_value(snapshot.clone())
            .unwrap_or_else(|_| project.timeline.clone());
    });

    let _ = save_project_inner(&state);

    Ok(json!({ "undone": desc }))
}

/// Redo the last undone operation.
#[tauri::command]
pub async fn redo(state: State<'_, AppState>) -> Result<Value, String> {
    let (desc, snapshot) = state.undo_manager.write()
        .map_err(|_| "Lock poisoned")?
        .redo()
        .ok_or("Nothing to redo")?;

    state.with_active_project_mut(|project| {
        project.timeline = serde_json::from_value(snapshot.clone())
            .unwrap_or_else(|_| project.timeline.clone());
    });

    let _ = save_project_inner(&state);

    Ok(json!({ "redone": desc }))
}

fn save_project_inner(state: &State<'_, AppState>) -> Result<(), String> {
    let timeline_path = state.with_active_project(|p| p.timeline_path.clone())
        .ok_or("No active project")?;
    let timeline_json = state.with_active_project(|p| {
        serde_json::to_string_pretty(&p.timeline)
    }).ok_or("No active project")??;
    std::fs::write(&timeline_path, &timeline_json)
        .map_err(|e| format!("Failed to save: {}", e))
}
```

- [ ] **Step 3: Update commands/mod.rs**

```rust
// crates/openscript-tauri/src/commands/mod.rs
pub mod project;
pub mod timeline;
// TODO: populate as tasks progress
pub mod transcript;
pub mod render;
pub mod assets;
pub mod voice;
pub mod motion;
pub mod system;

// Stub modules with empty content
```

Ensure all stub modules have at least one exported item or use `#![allow(dead_code)]`:

```rust
// crates/openscript-tauri/src/commands/transcript.rs (and others)
#![allow(dead_code)]
// Stub — will be implemented in Phase 2
```

- [ ] **Step 4: Register commands in main.rs**

```rust
// In main.rs, update invoke_handler:
.invoke_handler(tauri::generate_handler![
    // Project
    commands::project::create_project,
    commands::project::load_project,
    commands::project::list_projects,
    commands::project::save_project,
    // Timeline
    commands::timeline::add_segment,
    commands::timeline::get_timeline,
    commands::timeline::timeline_preview,
    commands::timeline::undo,
    commands::timeline::redo,
])
```

- [ ] **Step 5: Build and verify**

```bash
cd /home/ishanp/Documents/GitHub/openscript
cargo check -p openscript-tauri
```

- [ ] **Step 6: Commit**

```bash
git add crates/openscript-tauri/src/commands/ crates/openscript-tauri/src/main.rs crates/openscript-tauri/src/state/
git commit -m "feat: wire 5 core Tauri commands (project CRUD, timeline, undo/redo)"
```

---

### Task 1.5: Frontend — Tauri Invoke Layer + Project Store

**Files:**
- Create: `crates/openscript-tauri/src/frontend/src/lib/tauri.ts`
- Create: `crates/openscript-tauri/src/frontend/src/lib/utils.ts`
- Create: `crates/openscript-tauri/src/frontend/src/store/project.ts`
- Modify: `crates/openscript-tauri/src/frontend/src/App.tsx`

- [ ] **Step 1: Create Tauri invoke wrappers**

```typescript
// crates/openscript-tauri/src/frontend/src/lib/tauri.ts
import { invoke } from "@tauri-apps/api/core";

// Project commands
export async function createProject(sourceVideo: string) {
  return invoke<{ project_id: string; name: string; timeline_path: string }>(
    "create_project",
    { sourceVideo }
  );
}

export async function loadProject(projectId: string) {
  return invoke("load_project", { projectId });
}

export async function listProjects() {
  return invoke<
    Array<{ id: string; name: string; source_video: string; active: boolean }>
  >("list_projects");
}

export async function saveProject() {
  return invoke<{ saved: boolean; path: string }>("save_project");
}

// Timeline commands
export async function addSegment(
  start: number,
  end: number,
  caption: string,
  semanticRole?: string
) {
  return invoke<{ segment_id: string }>("add_segment", {
    start,
    end,
    caption,
    semanticRole,
  });
}

export async function getTimeline() {
  return invoke("get_timeline");
}

export async function timelinePreview() {
  return invoke<{
    total_duration_ms: number;
    segment_count: number;
    render_ready: boolean;
  }>("timeline_preview");
}

export async function undoAction() {
  return invoke<{ undone: string }>("undo");
}

export async function redoAction() {
  return invoke<{ redone: string }>("redo");
}
```

- [ ] **Step 2: Create utility functions**

```typescript
// crates/openscript-tauri/src/frontend/src/lib/utils.ts
import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

export function formatDuration(ms: number): string {
  const totalSeconds = Math.floor(ms / 1000);
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  const frames = Math.floor((ms % 1000) / 33.33); // 30fps
  return `${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}:${String(frames).padStart(2, "0")}`;
}

export function formatTimecode(seconds: number): string {
  const mins = Math.floor(seconds / 60);
  const secs = Math.floor(seconds % 60);
  return `${String(mins).padStart(2, "0")}:${String(secs).padStart(2, "0")}`;
}
```

- [ ] **Step 3: Create Project store**

```typescript
// crates/openscript-tauri/src/frontend/src/store/project.ts
import { create } from "zustand";
import * as api from "../lib/tauri";

export interface Segment {
  id: string;
  source_start_ms: number;
  source_end_ms: number;
  caption: string;
  semantic_role?: string;
  crossfade_ms?: number;
}

export interface ProjectState {
  // Data
  projectId: string | null;
  projectName: string;
  sourceVideo: string | null;
  segments: Segment[];
  isLoading: boolean;
  error: string | null;

  // Actions
  createProject: (sourceVideo: string) => Promise<void>;
  loadProject: (projectId: string) => Promise<void>;
  refreshTimeline: () => Promise<void>;
  addSegment: (start: number, end: number, caption: string, role?: string) => Promise<void>;
  undo: () => Promise<void>;
  redo: () => Promise<void>;
  save: () => Promise<void>;
}

export const useProjectStore = create<ProjectState>((set, get) => ({
  projectId: null,
  projectName: "Untitled",
  sourceVideo: null,
  segments: [],
  isLoading: false,
  error: null,

  createProject: async (sourceVideo: string) => {
    set({ isLoading: true, error: null });
    try {
      const result = await api.createProject(sourceVideo);
      set({
        projectId: result.project_id,
        projectName: result.name,
        sourceVideo,
        segments: [],
        isLoading: false,
      });
    } catch (e) {
      set({ error: String(e), isLoading: false });
    }
  },

  loadProject: async (projectId: string) => {
    set({ isLoading: true, error: null });
    try {
      await api.loadProject(projectId);
      await get().refreshTimeline();
      set({ projectId, isLoading: false });
    } catch (e) {
      set({ error: String(e), isLoading: false });
    }
  },

  refreshTimeline: async () => {
    try {
      const data = await api.getTimeline();
      const timeline = data as any;
      set({
        projectName: timeline.name || "Untitled",
        sourceVideo: timeline.source_video,
        segments: timeline.timeline?.segments || [],
      });
    } catch (e) {
      console.error("Failed to refresh timeline:", e);
    }
  },

  addSegment: async (start, end, caption, role) => {
    try {
      await api.addSegment(start, end, caption, role);
      await get().refreshTimeline();
    } catch (e) {
      set({ error: String(e) });
    }
  },

  undo: async () => {
    try {
      await api.undoAction();
      await get().refreshTimeline();
    } catch (e) {
      set({ error: String(e) });
    }
  },

  redo: async () => {
    try {
      await api.redoAction();
      await get().refreshTimeline();
    } catch (e) {
      set({ error: String(e) });
    }
  },

  save: async () => {
    try {
      await api.saveProject();
    } catch (e) {
      set({ error: String(e) });
    }
  },
}));
```

- [ ] **Step 4: Update App.tsx with basic layout**

```typescript
// crates/openscript-tauri/src/frontend/src/App.tsx
import { open } from "@tauri-apps/plugin-dialog";
import { useProjectStore } from "./store/project";
import { cn } from "./lib/utils";

function TopBar() {
  const { projectName, sourceVideo, createProject } = useProjectStore();

  const handleOpenVideo = async () => {
    const selected = await open({
      multiple: false,
      filters: [{ name: "Video", extensions: ["mp4", "mov", "avi", "mkv", "webm"] }],
    });
    if (selected && typeof selected === "string") {
      await createProject(selected);
    }
  };

  return (
    <header className="flex h-12 items-center justify-between border-b bg-background px-4">
      <div className="flex items-center gap-3">
        <h1 className="text-sm font-semibold">OpenScript</h1>
        {sourceVideo ? (
          <span className="text-xs text-muted-foreground truncate max-w-[300px]">
            {projectName}
          </span>
        ) : (
          <span className="text-xs text-muted-foreground">No project open</span>
        )}
      </div>
      <button
        onClick={handleOpenVideo}
        className="rounded-md bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90"
      >
        {sourceVideo ? "Open Another" : "Open Video"}
      </button>
    </header>
  );
}

function EmptyState() {
  const { createProject } = useProjectStore();

  return (
    <div className="flex flex-1 items-center justify-center">
      <div className="text-center">
        <h2 className="text-xl font-semibold mb-2">Welcome to OpenScript</h2>
        <p className="text-muted-foreground mb-4">
          Open a video file to start editing
        </p>
        <button
          onClick={async () => {
            const selected = await open({
              multiple: false,
              filters: [{ name: "Video", extensions: ["mp4", "mov", "avi"] }],
            });
            if (selected && typeof selected === "string") {
              await createProject(selected);
            }
          }}
          className="rounded-md bg-primary px-4 py-2 text-sm font-medium text-primary-foreground"
        >
          Choose Video
        </button>
      </div>
    </div>
  );
}

function App() {
  const { sourceVideo, error, segments } = useProjectStore();

  return (
    <div className="flex h-screen w-screen flex-col bg-background text-foreground">
      <TopBar />

      {error && (
        <div className="mx-4 mt-2 rounded-md bg-destructive/10 px-3 py-2 text-sm text-destructive">
          {error}
        </div>
      )}

      {!sourceVideo ? (
        <EmptyState />
      ) : (
        <div className="flex flex-1">
          {/* Left: Video Preview (placeholder for now) */}
          <div className="flex-1 flex items-center justify-center bg-black/5">
            <p className="text-muted-foreground text-sm">
              Video preview — {segments.length} segment(s)
            </p>
          </div>

          {/* Right: Segment list */}
          <div className="w-80 border-l overflow-y-auto">
            <div className="p-3 border-b">
              <h3 className="text-sm font-medium">Segments</h3>
            </div>
            {segments.map((seg) => (
              <div
                key={seg.id}
                className="px-3 py-2 border-b text-xs"
              >
                <div className="font-mono text-muted-foreground">{seg.id}</div>
                <div className="mt-1 truncate">{seg.caption}</div>
              </div>
            ))}
            {segments.length === 0 && (
              <div className="p-4 text-xs text-muted-foreground text-center">
                No segments yet. Add segments from the transcript or timeline.
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

export default App;
```

- [ ] **Step 5: Verify full app loads**

```bash
cd crates/openscript-tauri/src/frontend && npm run build
cd /home/ishanp/Documents/GitHub/openscript && cargo check -p openscript-tauri
```

- [ ] **Step 6: Commit**

```bash
git add crates/openscript-tauri/src/frontend/src/
git commit -m "feat: frontend invoke layer, project store, basic app layout"
```

---

## Phase 2: Critical Backend Gaps

### Task 2.1: `system.capabilities` Tool

**Files:**
- Create: `crates/openscript-tauri/src/commands/system.rs`
- Modify: `crates/openscript-tauri/src/main.rs`

- [ ] **Step 1: Implement system.capabilities command**

```rust
// crates/openscript-tauri/src/commands/system.rs
use serde_json::{json, Value};
use std::path::PathBuf;
use tauri::State;

use crate::state::AppState;

/// Check which OpenScript subsystems are available.
/// Call this at app startup to know what features are enabled.
#[tauri::command]
pub async fn system_capabilities(state: State<'_, AppState>) -> Result<Value, String> {
    // Check voicebox
    let voicebox_available = {
        let client = reqwest::Client::new();
        match client.get(format!("{}/health", state.tts_url)).timeout(std::time::Duration::from_secs(2)).send().await {
            Ok(resp) => {
                if let Ok(body) = resp.json::<serde_json::Value>().await {
                    let model_loaded = body.get("model_loaded").and_then(|v| v.as_bool()).unwrap_or(false);
                    json!({
                        "available": true,
                        "model_loaded": model_loaded,
                        "url": state.tts_url,
                    })
                } else {
                    json!({ "available": false, "reason": "Voicebox responded but returned invalid JSON" })
                }
            }
            Err(_) => json!({ "available": false, "reason": format!("Cannot reach voicebox at {}", state.tts_url) }),
        }
    };

    // Check Pexels
    let pexels_available = json!({
        "available": state.pexels_api_key.is_some(),
        "reason": if state.pexels_api_key.is_none() { "PEXELS_API_KEY not set" } else { "Ready" },
    });

    // Check SFX library
    let sfx_path = state.assets_base_path.join("SFX");
    let sfx_count = if sfx_path.exists() {
        std::fs::read_dir(&sfx_path).map(|d| d.count()).unwrap_or(0)
    } else { 0 };

    // Check music library
    let music_path = state.assets_base_path.join("Music");
    let music_count = if music_path.exists() {
        std::fs::read_dir(&music_path).map(|d| d.count()).unwrap_or(0)
    } else { 0 };

    // Check transcription engine
    let transcription_available = json!({
        "available": true, // Apex is assumed if conda env exists
        "engine": "apex",
    });

    // Check FFmpeg
    let ffmpeg_available = {
        let output = std::process::Command::new("ffmpeg").arg("-version").output();
        match output {
            Ok(o) if o.status.success() => json!({ "available": true }),
            _ => json!({ "available": false, "reason": "ffmpeg not found in PATH" }),
        }
    };

    Ok(json!({
        "voicebox": voicebox_available,
        "pexels": pexels_available,
        "sfx_library": { "available": sfx_count > 0, "indexed_count": sfx_count },
        "music_library": { "available": music_count > 0, "indexed_count": music_count },
        "transcription": transcription_available,
        "ffmpeg": ffmpeg_available,
    }))
}
```

Add `reqwest` to `Cargo.toml`:
```toml
reqwest = { version = "0.12", features = ["json"] }
```

- [ ] **Step 2: Register in main.rs**

Add to `invoke_handler`:
```rust
commands::system::system_capabilities,
```

- [ ] **Step 3: Commit**

```bash
git add crates/openscript-tauri/src/commands/system.rs crates/openscript-tauri/Cargo.toml
git commit -m "feat: system.capabilities command for subsystem availability check"
```

---

### Task 2.2: Speaker Detection & Filler Word Detection

**Files:**
- Modify: `crates/openscript-core/src/srt/mod.rs`
- Modify: `crates/openscript-core/src/types.rs`
- Create: `crates/openscript-core/src/transcript/analysis.rs`

- [ ] **Step 1: Create transcript analysis module**

```rust
// crates/openscript-core/src/transcript/analysis.rs
//! Transcript analysis: speaker detection, filler word identification, silence gaps.

use serde::{Deserialize, Serialize};

/// Common filler words/phrases to detect in transcripts.
const FILLER_WORDS: &[&str] = &[
    "um", "uh", "uhh", "umm", "er", "like", "you know", "basically",
    "actually", "literally", "so", "right", "okay", "well",
];

/// A detected filler word in the transcript.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FillerWord {
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub segment_id: String,
}

/// Analysis result for a transcript.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptAnalysis {
    pub filler_words: Vec<FillerWord>,
    pub filler_word_count: usize,
    pub total_words: usize,
    pub filler_percentage: f64,
    pub word_count: u64,
    pub estimated_duration_s: f64,
    pub segments_analyzed: usize,
}

/// Detect filler words in caption text.
pub fn detect_filler_words(
    segments: &[(String, u64, u64, String)], // (segment_id, start_ms, end_ms, text)
) -> TranscriptAnalysis {
    let mut filler_words = Vec::new();
    let mut total_words = 0;

    for (segment_id, start_ms, _end_ms, text) in segments {
        let words: Vec<&str> = text.split_whitespace().collect();
        total_words += words.len();

        for word in &words {
            let lower = word.to_lowercase().trim_matches(|c: char| !c.is_alphabetic());
            if FILLER_WORDS.contains(&lower) {
                filler_words.push(FillerWord {
                    text: word.to_string(),
                    start_ms: *start_ms,
                    end_ms: *start_ms + 500, // Approximate
                    segment_id: segment_id.clone(),
                });
            }
        }
    }

    let filler_percentage = if total_words > 0 {
        (filler_words.len() as f64 / total_words as f64) * 100.0
    } else {
        0.0
    };

    TranscriptAnalysis {
        filler_word_count: filler_words.len(),
        filler_words,
        total_words,
        filler_percentage,
        word_count: total_words as u64,
        estimated_duration_s: 0.0, // Would come from SRT
        segments_analyzed: segments.len(),
    }
}

/// Detect potential speaker changes based on text patterns.
/// This is a heuristic — true speaker detection requires audio analysis.
pub fn detect_speaker_changes(segments: &[(String, String)]) -> Vec<(String, String)> {
    // Returns (segment_id, suggested_speaker_label)
    // Heuristic: segments starting with question marks or different sentiment
    // are potential speaker changes. Real implementation needs audio diarization.
    segments.iter().map(|(id, text)| {
        let trimmed = text.trim();
        if trimmed.starts_with('?') || trimmed.starts_with("Wait") || trimmed.starts_with("Hmm") {
            (id.clone(), "Speaker B".to_string())
        } else {
            (id.clone(), "Speaker A".to_string())
        }
    }).collect()
}

/// Remove filler words from text, returning cleaned text.
pub fn remove_filler_words(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    words.into_iter()
        .filter(|w| {
            let lower = w.to_lowercase().trim_matches(|c: char| !c.is_alphabetic());
            !FILLER_WORDS.contains(&lower)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_filler_words() {
        let segments = vec![
            ("seg_001".to_string(), 0, 5000, "um well I think like basically".to_string()),
            ("seg_002".to_string(), 5000, 10000, "the project is actually going well".to_string()),
        ];

        let analysis = detect_filler_words(&segments);
        assert_eq!(analysis.filler_word_count, 3); // um, like, basically
        assert!(analysis.filler_percentage > 0.0);
    }

    #[test]
    fn test_remove_filler_words() {
        let text = "um well I think like basically yes";
        let cleaned = remove_filler_words(text);
        assert_eq!(cleaned, "well I think yes");
    }

    #[test]
    fn test_detect_speaker_changes() {
        let segments = vec![
            ("seg_001".to_string(), "Hello world".to_string()),
            ("seg_002".to_string(), "?What do you think".to_string()),
        ];
        let speakers = detect_speaker_changes(&segments);
        assert_eq!(speakers[0].1, "Speaker A");
        assert_eq!(speakers[1].1, "Speaker B");
    }
}
```

- [ ] **Step 2: Add module to openscript-core**

```rust
// crates/openscript-core/src/lib.rs — add:
pub mod transcript;

// crates/openscript-core/src/transcript/mod.rs — create:
pub mod analysis;
pub use analysis::*;
```

- [ ] **Step 3: Commit**

```bash
git add crates/openscript-core/src/transcript/
git commit -m "feat: transcript analysis — filler word detection, speaker heuristic"
```

---

### Task 2.3: Split Segment + Render Fixes

**Files:**
- Modify: `crates/openscript-tauri/src/commands/timeline.rs`
- Modify: `crates/openscript-ffmpeg/src/render.rs` (placeholder b-roll fix)

- [ ] **Step 1: Add split_segment command**

```rust
// Add to crates/openscript-tauri/src/commands/timeline.rs

/// Split a segment at a given timestamp (relative to source video).
/// Creates two segments: original start → split point, split point → original end.
#[tauri::command]
pub async fn split_segment(
    state: State<'_, AppState>,
    segment_id: String,
    split_time_s: f64,
) -> Result<Value, String> {
    let split_ms = (split_time_s * 1000.0) as u64;

    let result = state.with_active_timeline_mut(|timeline| {
        let seg_idx = timeline.segments.iter().position(|s| s.id == segment_id);
        match seg_idx {
            Some(idx) => {
                let seg = &timeline.segments[idx];
                if split_ms <= seg.source_start_ms || split_ms >= seg.source_end_ms {
                    return Err(format!(
                        "Split time {:.2}s is outside segment range [{:.2}s - {:.2}s]",
                        split_time_s,
                        seg.source_start_ms as f64 / 1000.0,
                        seg.source_end_ms as f64 / 1000.0
                    ));
                }

                let new_id = format!("seg_{:03}", timeline.segments.len() + 1);
                let original_caption = seg.caption.clone();
                let original_role = seg.semantic_role.clone();
                let original_crossfade = seg.crossfade_ms;

                // Modify original: start → split
                timeline.segments[idx].source_end_ms = split_ms;

                // Create new segment: split → end
                let new_seg = openscript_core::timeline::schema::Segment {
                    id: new_id.clone(),
                    source_start_ms: split_ms,
                    source_end_ms: seg.source_end_ms,
                    caption: original_caption,
                    semantic_role: original_role,
                    crossfade_ms: original_crossfade,
                    ..Default::default()
                };
                timeline.segments.insert(idx + 1, new_seg);

                Ok(new_id)
            }
            None => Err(format!("Segment not found: {}", segment_id)),
        }
    }).ok_or("No active project")??;

    let _ = save_project_inner(&state);

    Ok(json!({ "segment_id": result, "split": true }))
}
```

Register in main.rs:
```rust
commands::timeline::split_segment,
```

- [ ] **Step 2: Fix placeholder b-roll render crash**

```rust
// In crates/openscript-ffmpeg/src/render.rs
// Find the b-roll filter graph building section and add:

let broll_events: Vec<_> = timeline.tracks.get(&TrackType::Broll)
    .map(|events| events.iter().filter(|e| e.asset_id != "placeholder").collect())
    .unwrap_or_default();
```

- [ ] **Step 3: Commit**

```bash
git add crates/openscript-tauri/src/commands/timeline.rs crates/openscript-ffmpeg/src/render.rs
git commit -m "feat: split_segment command + fix placeholder b-roll render crash"
```

---

## Phase 3: Text-Based Transcript Editor

### Task 3.1: Transcription Commands + Tauri Integration

**Files:**
- Modify: `crates/openscript-tauri/src/commands/transcript.rs`
- Modify: `crates/openscript-tauri/src/main.rs`

- [ ] **Step 1: Implement transcript commands**

```rust
// crates/openscript-tauri/src/commands/transcript.rs
use openscript_core::srt;
use openscript_core::transcript::analysis::{detect_filler_words, remove_filler_words, TranscriptAnalysis};
use serde_json::{json, Value};
use tauri::State;
use std::path::PathBuf;

use crate::state::AppState;

/// Transcribe a source video using Apex.
#[tauri::command(async)]
pub async fn transcribe_video(
    state: State<'_, AppState>,
    video_path: String,
) -> Result<Value, String> {
    let app = tauri::async_runtime::spawn_blocking(move || {
        // Call openscript_transcribe directly
        openscript_transcribe::transcribe(&video_path, None)
    }).await.map_err(|e| format!("Transcription panicked: {}", e))??;

    // Store transcript path in active project
    if let Some(srt_path) = app.get("output_srt_path").and_then(|v| v.as_str()) {
        state.with_active_project_mut(|project| {
            project.transcript_path = Some(srt_path.to_string());
        });
    }

    Ok(app)
}

/// Read and parse an SRT file into segments.
#[tauri::command]
pub async fn read_transcript(srt_path: String) -> Result<Value, String> {
    let entries = srt::parse(&srt_path)
        .map_err(|e| format!("Failed to parse SRT: {}", e))?;

    let segments: Vec<Value> = entries.iter().map(|e| {
        json!({
            "index": e.index,
            "start": e.start,
            "end": e.end,
            "text": e.text,
        })
    }).collect();

    Ok(json!({ "count": segments.len(), "segments": segments }))
}

/// Group word-level SRT into phrase-level for readable editing.
#[tauri::command]
pub async fn prepare_transcript(
    word_srt_path: String,
    max_words: Option<usize>,
    max_chars: Option<usize>,
) -> Result<Value, String> {
    let groups = srt::group_words(&word_srt_path, max_words.unwrap_or(10), max_chars.unwrap_or(64), 0.6)
        .map_err(|e| format!("Failed to group words: {}", e))?;

    let segments: Vec<Value> = groups.iter().map(|g| {
        json!({
            "start": g.start,
            "end": g.end,
            "text": g.text,
        })
    }).collect();

    Ok(json!({ "count": segments.len(), "segments": segments }))
}

/// Analyze transcript for filler words.
#[tauri::command]
pub async fn analyze_transcript(srt_path: String) -> Result<Value, String> {
    let entries = srt::parse(&srt_path)
        .map_err(|e| format!("Failed to parse SRT: {}", e))?;

    let segments: Vec<_> = entries.iter()
        .map(|e| (e.index.to_string(), (e.start * 1000.0) as u64, (e.end * 1000.0) as u64, e.text.clone()))
        .collect();

    let analysis = detect_filler_words(&segments);

    Ok(json!({
        "filler_word_count": analysis.filler_word_count,
        "total_words": analysis.total_words,
        "filler_percentage": analysis.filler_percentage,
        "filler_words": analysis.filler_words,
    }))
}

/// Remove filler words from the transcript, returning cleaned text.
#[tauri::command]
pub async fn remove_filler_words_from_text(text: String) -> Result<Value, String> {
    let cleaned = remove_filler_words(&text);
    Ok(json!({ "original": text, "cleaned": cleaned }))
}

/// Apply edited SRT to video: build EDL and render.
#[tauri::command(async)]
pub async fn apply_transcript_edit(
    state: State<'_, AppState>,
    video_path: String,
    edited_segments: Vec<Value>,
) -> Result<Value, String> {
    // Build EDL from edited segments
    let edl_path = PathBuf::from("artifacts/edited_edl.json");

    // Write EDL from segments
    let edl_segments: Vec<_> = edited_segments.iter().filter_map(|seg| {
        Some(serde_json::json!({
            "start": seg["start"].as_f64()?,
            "end": seg["end"].as_f64()?,
            "caption": seg["text"].as_str()?,
        }))
    }).collect();

    let edl = json!({
        "version": "1.0",
        "segments": edl_segments,
        "crossfade_ms": 120,
    });

    let edl_content = serde_json::to_string_pretty(&edl)
        .map_err(|e| format!("Failed to serialize EDL: {}", e))?;
    std::fs::write(&edl_path, &edl_content)
        .map_err(|e| format!("Failed to write EDL: {}", e))?;

    // Render using FFmpeg
    let output_path = PathBuf::from("artifacts/edited_output.mp4");
    openscript_ffmpeg::render::render_edl(&video_path, &edl_path.to_string_lossy(), &output_path.to_string_lossy(), true, 20, 30)
        .map_err(|e| format!("Render failed: {}", e))?;

    Ok(json!({
        "output_path": output_path.to_string_lossy(),
        "segments_count": edl_segments.len(),
    }))
}
```

Note: This assumes `openscript_transcribe` and `openscript_ffmpeg` expose the functions we need. If they don't, we'll need to adapt the crate APIs or use subprocess calls.

- [ ] **Step 2: Register commands**

Add to main.rs `invoke_handler`:
```rust
// Transcript
commands::transcript::transcribe_video,
commands::transcript::read_transcript,
commands::transcript::prepare_transcript,
commands::transcript::analyze_transcript,
commands::transcript::remove_filler_words_from_text,
commands::transcript::apply_transcript_edit,
```

- [ ] **Step 3: Commit**

```bash
git add crates/openscript-tauri/src/commands/transcript.rs
git commit -m "feat: transcript commands (transcribe, read, prepare, analyze, edit)"
```

---

### Task 3.2: Transcript Editor UI (TipTap)

**Files:**
- Create: `crates/openscript-tauri/src/frontend/src/store/transcript.ts`
- Create: `crates/openscript-tauri/src/frontend/src/components/transcript/TranscriptEditor.tsx`
- Create: `crates/openscript-tauri/src/frontend/src/components/transcript/TranscriptSegment.tsx`
- Create: `crates/openscript-tauri/src/frontend/src/components/transcript/WordToken.tsx`
- Modify: `crates/openscript-tauri/src/frontend/src/App.tsx`

- [ ] **Step 1: Create Transcript store**

```typescript
// crates/openscript-tauri/src/frontend/src/store/transcript.ts
import { create } from "zustand";
import * as api from "../lib/tauri";

export interface TranscriptEntry {
  index: number;
  start: number;
  end: number;
  text: string;
}

export interface FillerWord {
  text: string;
  start_ms: number;
  end_ms: number;
  segment_id: string;
}

export interface TranscriptState {
  entries: TranscriptEntry[];
  isTranscribing: boolean;
  transcriptionProgress: number;
  fillerAnalysis: {
    filler_word_count: number;
    total_words: number;
    filler_percentage: number;
    filler_words: FillerWord[];
  } | null;
  isEditing: boolean;

  // Actions
  transcribe: (videoPath: string) => Promise<void>;
  loadTranscript: (srtPath: string) => Promise<void>;
  prepareTranscript: (wordSrtPath: string) => Promise<void>;
  analyzeFillerWords: (srtPath: string) => Promise<void>;
  removeFillerWords: (text: string) => Promise<string>;
  applyEdit: (videoPath: string, segments: any[]) => Promise<void>;
}

export const useTranscriptStore = create<TranscriptState>((set) => ({
  entries: [],
  isTranscribing: false,
  transcriptionProgress: 0,
  fillerAnalysis: null,
  isEditing: false,

  transcribe: async (videoPath: string) => {
    set({ isTranscribing: true, transcriptionProgress: 0 });
    try {
      const result = await api.transcribeVideo(videoPath);
      set({ isTranscribing: false, transcriptionProgress: 100 });
      if ((result as any).output_srt_path) {
        set((state) => ({ ...state })); // Trigger refresh
      }
    } catch (e) {
      set({ isTranscribing: false });
      throw e;
    }
  },

  loadTranscript: async (srtPath: string) => {
    const result = await api.readTranscript(srtPath);
    set({ entries: (result as any).segments || [] });
  },

  prepareTranscript: async (wordSrtPath: string) => {
    const result = await api.prepareTranscript(wordSrtPath);
    set({ entries: (result as any).segments || [] });
  },

  analyzeFillerWords: async (srtPath: string) => {
    const result = await api.analyzeTranscript(srtPath);
    set({ fillerAnalysis: result as any });
  },

  removeFillerWords: async (text: string) => {
    const result = await api.removeFillerWordsFromText(text);
    return (result as any).cleaned;
  },

  applyEdit: async (videoPath: string, segments: any[]) => {
    set({ isEditing: true });
    try {
      await api.applyTranscriptEdit(videoPath, segments);
      set({ isEditing: false });
    } catch (e) {
      set({ isEditing: false });
      throw e;
    }
  },
}));
```

- [ ] **Step 2: Create TranscriptEditor component**

```typescript
// crates/openscript-tauri/src/frontend/src/components/transcript/TranscriptEditor.tsx
import { useEditor, EditorContent } from "@tiptap/react";
import StarterKit from "@tiptap/starter-kit";
import { useTranscriptStore } from "../../store/transcript";
import { useProjectStore } from "../../store/project";
import { cn } from "../../lib/utils";
import { Eraser, Loader2, Sparkles } from "lucide-react";
import { useState } from "react";

export function TranscriptEditor() {
  const { entries, fillerAnalysis, removeFillerWords, analyzeFillerWords } =
    useTranscriptStore();
  const { sourceVideo } = useProjectStore();
  const [highlightFillers, setHighlightFillers] = useState(false);

  const editor = useEditor({
    extensions: [StarterKit],
    content: entries.map((e) => `<p data-start="${e.start}" data-end="${e.end}">${e.text}</p>`).join(""),
    editable: true,
    editorProps: {
      attributes: {
        class: "prose prose-sm max-w-none focus:outline-none min-h-[200px] p-4",
      },
    },
  });

  const handleRemoveFillers = async () => {
    if (!editor) return;
    const text = editor.getText();
    const cleaned = await removeFillerWords(text);
    editor.commands.setContent(cleaned);
  };

  if (entries.length === 0) {
    return (
      <div className="flex flex-1 items-center justify-center text-muted-foreground text-sm">
        No transcript yet. Transcribe a video to see text here.
      </div>
    );
  }

  return (
    <div className="flex flex-col h-full">
      {/* Toolbar */}
      <div className="flex items-center gap-2 px-3 py-2 border-b">
        {fillerAnalysis && fillerAnalysis.filler_word_count > 0 && (
          <>
            <button
              onClick={() => setHighlightFillers(!highlightFillers)}
              className={cn(
                "flex items-center gap-1 rounded-md px-2 py-1 text-xs",
                highlightFillers ? "bg-yellow-100 text-yellow-800" : "bg-secondary text-secondary-foreground"
              )}
            >
              <Sparkles className="w-3 h-3" />
              {fillerAnalysis.filler_word_count} filler words
            </button>
            <button
              onClick={handleRemoveFillers}
              className="flex items-center gap-1 rounded-md px-2 py-1 text-xs bg-destructive/10 text-destructive hover:bg-destructive/20"
            >
              <Eraser className="w-3 h-3" />
              Remove all
            </button>
          </>
        )}
      </div>

      {/* Content */}
      <div className="flex-1 overflow-y-auto">
        <EditorContent editor={editor} />
      </div>
    </div>
  );
}
```

- [ ] **Step 3: Create WordToken component**

```typescript
// crates/openscript-tauri/src/frontend/src/components/transcript/WordToken.tsx
import { cn } from "../../lib/utils";

const FILLER_WORDS = new Set([
  "um", "uh", "uhh", "umm", "er", "like", "you know", "basically",
  "actually", "literally", "so", "right", "okay", "well",
]);

interface WordTokenProps {
  word: string;
  onClick?: () => void;
  highlightFillers?: boolean;
}

export function WordToken({ word, onClick, highlightFillers }: WordTokenProps) {
  const isFiller = FILLER_WORDS.has(word.toLowerCase().replace(/[^a-z]/g, ""));

  return (
    <span
      className={cn(
        "cursor-pointer rounded px-0.5 py-0 transition-colors",
        isFiller && highlightFillers && "bg-yellow-200 text-yellow-900 line-through",
        !isFiller && "hover:bg-secondary",
      )}
      onClick={onClick}
    >
      {word}{" "}
    </span>
  );
}
```

- [ ] **Step 4: Integrate into App layout**

Update `App.tsx` to include a 3-panel layout:
- Left: Transcript Editor
- Center: Video Preview + Timeline
- Right: Asset Browser (placeholder for now)

- [ ] **Step 5: Commit**

```bash
git add crates/openscript-tauri/src/frontend/src/components/transcript/ crates/openscript-tauri/src/frontend/src/store/transcript.ts
git commit -m "feat: TipTap transcript editor with filler word detection"
```

---

## Phase 4: Timeline Editor

### Task 4.1: Multi-Track Timeline UI

**Files:**
- Create: `crates/openscript-tauri/src/frontend/src/components/timeline/TimelineEditor.tsx`
- Create: `crates/openscript-tauri/src/frontend/src/components/timeline/TrackRow.tsx`
- Create: `crates/openscript-tauri/src/frontend/src/components/timeline/SegmentBlock.tsx`
- Create: `crates/openscript-tauri/src/frontend/src/components/timeline/Playhead.tsx`
- Create: `crates/openscript-tauri/src/frontend/src/components/timeline/TimeRuler.tsx`
- Create: `crates/openscript-tauri/src/frontend/src/components/timeline/WaveformDisplay.tsx`
- Create: `crates/openscript-tauri/src/frontend/src/hooks/usePlayback.ts`
- Create: `crates/openscript-tauri/src/frontend/src/store/editor.ts`

- [ ] **Step 1: Create Editor store**

```typescript
// crates/openscript-tauri/src/frontend/src/store/editor.ts
import { create } from "zustand";

export interface EditorState {
  playbackPosition: number; // ms
  isPlaying: boolean;
  zoom: number; // pixels per second
  selectedSegmentId: string | null;
  selectedTrack: string | null;
  activePanel: "transcript" | "timeline" | "assets" | "ai";

  setPlaybackPosition: (ms: number) => void;
  setIsPlaying: (playing: boolean) => void;
  setZoom: (zoom: number) => void;
  setSelectedSegment: (id: string | null) => void;
  setSelectedTrack: (track: string | null) => void;
  setActivePanel: (panel: "transcript" | "timeline" | "assets" | "ai") => void;
}

export const useEditorStore = create<EditorState>((set) => ({
  playbackPosition: 0,
  isPlaying: false,
  zoom: 100,
  selectedSegmentId: null,
  selectedTrack: null,
  activePanel: "transcript",

  setPlaybackPosition: (ms) => set({ playbackPosition: ms }),
  setIsPlaying: (isPlaying) => set({ isPlaying }),
  setZoom: (zoom) => set({ zoom }),
  setSelectedSegment: (selectedSegmentId) => set({ selectedSegmentId }),
  setSelectedTrack: (selectedTrack) => set({ selectedTrack }),
  setActivePanel: (activePanel) => set({ activePanel }),
}));
```

- [ ] **Step 2: Create TimelineEditor**

```typescript
// crates/openscript-tauri/src/frontend/src/components/timeline/TimelineEditor.tsx
import { useRef, useCallback } from "react";
import { useProjectStore } from "../../store/project";
import { useEditorStore } from "../../store/editor";
import { TimeRuler } from "./TimeRuler";
import { TrackRow } from "./TrackRow";
import { Playhead } from "./Playhead";
import { cn } from "../../lib/utils";
import { ZoomIn, ZoomOut } from "lucide-react";

const TRACKS = [
  { id: "dialogue", label: "Dialogue", color: "bg-blue-500" },
  { id: "voiceover", label: "Voiceover", color: "bg-purple-500" },
  { id: "captions", label: "Captions", color: "bg-yellow-500" },
  { id: "broll", label: "B-Roll", color: "bg-green-500" },
  { id: "music", label: "Music", color: "bg-pink-500" },
  { id: "sfx", label: "SFX", color: "bg-orange-500" },
];

export function TimelineEditor() {
  const { segments } = useProjectStore();
  const { zoom, setZoom, playbackPosition, setPlaybackPosition } = useEditorStore();
  const containerRef = useRef<HTMLDivElement>(null);

  const handleTimeRulerClick = useCallback(
    (ms: number) => {
      setPlaybackPosition(ms);
    },
    [setPlaybackPosition]
  );

  const totalDurationMs = segments.reduce(
    (max, s) => Math.max(max, s.source_end_ms),
    0
  );
  const totalWidth = Math.max(totalDurationMs / 1000 * zoom, 800);

  return (
    <div className="flex flex-col h-full bg-[#1a1a2e] text-white">
      {/* Zoom controls */}
      <div className="flex items-center justify-between px-3 py-1 border-b border-white/10">
        <span className="text-xs text-white/60">Timeline</span>
        <div className="flex items-center gap-1">
          <button
            onClick={() => setZoom(Math.max(20, zoom - 20))}
            className="p-1 rounded hover:bg-white/10"
          >
            <ZoomOut className="w-3 h-3" />
          </button>
          <span className="text-xs text-white/40 w-12 text-center">{zoom}px/s</span>
          <button
            onClick={() => setZoom(Math.min(500, zoom + 20))}
            className="p-1 rounded hover:bg-white/10"
          >
            <ZoomIn className="w-3 h-3" />
          </button>
        </div>
      </div>

      {/* Timeline area */}
      <div ref={containerRef} className="flex-1 overflow-x-auto overflow-y-auto relative">
        <div style={{ width: totalWidth, minWidth: "100%" }} className="relative">
          {/* Time ruler */}
          <div className="sticky top-0 z-10 bg-[#1a1a2e] border-b border-white/10">
            <TimeRuler
              durationMs={totalDurationMs}
              zoom={zoom}
              onClick={handleTimeRulerClick}
            />
          </div>

          {/* Playhead */}
          <Playhead positionMs={playbackPosition} zoom={zoom} />

          {/* Track rows */}
          <div className="py-2">
            {TRACKS.map((track) => (
              <TrackRow
                key={track.id}
                trackId={track.id}
                label={track.label}
                color={track.color}
                segments={track.id === "dialogue" ? segments : []}
                zoom={zoom}
              />
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}
```

- [ ] **Step 3: Create TrackRow**

```typescript
// crates/openscript-tauri/src/frontend/src/components/timeline/TrackRow.tsx
import { Segment } from "../../store/project";
import { SegmentBlock } from "./SegmentBlock";
import { cn } from "../../lib/utils";

interface TrackRowProps {
  trackId: string;
  label: string;
  color: string;
  segments: Segment[];
  zoom: number;
}

export function TrackRow({ trackId, label, color, segments, zoom }: TrackRowProps) {
  return (
    <div className="flex h-12 border-b border-white/5">
      {/* Label */}
      <div className="w-24 flex-shrink-0 flex items-center px-2 text-xs text-white/60 bg-white/5 border-r border-white/10">
        <div className={cn("w-2 h-2 rounded-full mr-2", color)} />
        {label}
      </div>

      {/* Content area */}
      <div className="flex-1 relative">
        {segments.map((seg) => (
          <SegmentBlock
            key={seg.id}
            segment={seg}
            zoom={zoom}
            color={color}
          />
        ))}
      </div>
    </div>
  );
}
```

- [ ] **Step 4: Create SegmentBlock**

```typescript
// crates/openscript-tauri/src/frontend/src/components/timeline/SegmentBlock.tsx
import { Segment } from "../../store/project";
import { useEditorStore } from "../../store/editor";
import { cn } from "../../lib/utils";

interface SegmentBlockProps {
  segment: Segment;
  zoom: number;
  color: string;
}

export function SegmentBlock({ segment, zoom, color }: SegmentBlockProps) {
  const { selectedSegmentId, setSelectedSegment, setPlaybackPosition } = useEditorStore();

  const startMs = segment.source_start_ms;
  const durationMs = segment.source_end_ms - segment.source_start_ms;
  const left = (startMs / 1000) * zoom;
  const width = (durationMs / 1000) * zoom;

  const isSelected = selectedSegmentId === segment.id;

  return (
    <div
      className={cn(
        "absolute top-1 h-10 rounded-md px-2 flex items-center cursor-pointer",
        "text-xs text-white truncate select-none",
        color,
        isSelected && "ring-2 ring-white",
        "hover:brightness-110"
      )}
      style={{ left: `${left}px`, width: `${Math.max(width, 20)}px` }}
      onClick={(e) => {
        e.stopPropagation();
        setSelectedSegment(segment.id);
        setPlaybackPosition(segment.source_start_ms);
      }}
      title={segment.caption}
    >
      {width > 60 && segment.caption}
    </div>
  );
}
```

- [ ] **Step 5: Create Playhead**

```typescript
// crates/openscript-tauri/src/frontend/src/components/timeline/Playhead.tsx
import { formatDuration } from "../../lib/utils";

interface PlayheadProps {
  positionMs: number;
  zoom: number;
}

export function Playhead({ positionMs, zoom }: PlayheadProps) {
  const left = (positionMs / 1000) * zoom;

  return (
    <div
      className="absolute top-0 bottom-0 z-20 pointer-events-none"
      style={{ left: `${left}px` }}
    >
      {/* Triangle handle */}
      <div className="w-0 h-0 border-l-[6px] border-r-[6px] border-t-[8px] border-l-transparent border-r-transparent border-t-red-500" />
      {/* Vertical line */}
      <div className="w-0.5 bg-red-500 h-full -ml-[1px]" />
      {/* Time label */}
      <div className="absolute -translate-x-1/2 top-3 bg-red-500 text-white text-[10px] px-1 rounded">
        {formatDuration(positionMs)}
      </div>
    </div>
  );
}
```

- [ ] **Step 6: Create TimeRuler**

```typescript
// crates/openscript-tauri/src/frontend/src/components/timeline/TimeRuler.tsx
import { formatTimecode } from "../../lib/utils";

interface TimeRulerProps {
  durationMs: number;
  zoom: number;
  onClick?: (ms: number) => void;
}

export function TimeRuler({ durationMs, zoom, onClick }: TimeRulerProps) {
  const totalSeconds = Math.ceil(durationMs / 1000);
  const ticks = [];
  const interval = zoom > 200 ? 1 : zoom > 100 ? 5 : 10; // seconds per tick

  for (let s = 0; s <= totalSeconds; s += interval) {
    ticks.push(
      <div
        key={s}
        className="absolute flex flex-col items-center"
        style={{ left: `${s * zoom}px` }}
        onClick={() => onClick?.(s * 1000)}
      >
        <div className="w-px h-3 bg-white/30" />
        <span className="text-[9px] text-white/40 mt-0.5 select-none">
          {formatTimecode(s)}
        </span>
      </div>
    );
  }

  return (
    <div className="relative h-7 bg-[#1a1a2e]">
      {ticks}
    </div>
  );
}
```

- [ ] **Step 7: Commit**

```bash
git add crates/openscript-tauri/src/frontend/src/components/timeline/ crates/openscript-tauri/src/frontend/src/store/editor.ts crates/openscript-tauri/src/frontend/src/hooks/
git commit -m "feat: multi-track timeline editor with playhead, zoom, track rows"
```

---

## Phase 5: Asset Libraries

### Task 5.1: B-Roll, Music, SFX Browser

**Files:**
- Create: `crates/openscript-tauri/src/frontend/src/store/assets.ts`
- Create: `crates/openscript-tauri/src/frontend/src/components/assets/AssetBrowser.tsx`
- Create: `crates/openscript-tauri/src/frontend/src/components/assets/BrollGrid.tsx`
- Create: `crates/openscript-tauri/src/frontend/src/components/assets/MusicList.tsx`
- Create: `crates/openscript-tauri/src/frontend/src/components/assets/SFXList.tsx`

- [ ] **Step 1: Create Assets store**

```typescript
// crates/openscript-tauri/src/frontend/src/store/assets.ts
import { create } from "zustand";
import * as api from "../../lib/tauri";

export interface BrollResult {
  concept: string;
  videos: Array<{ id: string; width: number; height: number; url: string }>;
  cached_path?: string;
}

export interface MusicResult {
  title: string;
  artist: string;
  path: string;
  duration_ms: number;
  mood: string;
  energy: string;
}

export interface SFXResult {
  id: string;
  filename: string;
  path: string;
  category: string;
  editorial_role: string;
  duration_ms: number;
}

export interface AssetState {
  brollResults: BrollResult[];
  musicResults: MusicResult[];
  sfxResults: SFXResult[];
  isSearching: boolean;

  searchBroll: (concepts: string[], download?: boolean) => Promise<void>;
  searchMusic: (mood?: string, energy?: string) => Promise<void>;
  searchSFX: (query?: string, role?: string) => Promise<void>;
  assignBroll: (concept: string, positionMs: number, durationMs: number) => Promise<void>;
  assignMusic: (mood: string, energy: string) => Promise<void>;
  assignSFX: (role: string, positionMs: number) => Promise<void>;
}

export const useAssetStore = create<AssetState>((set) => ({
  brollResults: [],
  musicResults: [],
  sfxResults: [],
  isSearching: false,

  searchBroll: async (concepts, download = false) => {
    set({ isSearching: true });
    try {
      const results = await api.brollFetch(concepts, download);
      set({ brollResults: results as any, isSearching: false });
    } catch {
      set({ isSearching: false });
    }
  },

  searchMusic: async (mood, energy) => {
    const results = await api.musicSearch(mood, energy);
    set({ musicResults: results as any });
  },

  searchSFX: async (query, role) => {
    const results = await api.sfxSearch(query, role);
    set({ sfxResults: results as any });
  },

  assignBroll: async (concept, positionMs, durationMs) => {
    await api.brollAssign(concept, positionMs, durationMs);
  },

  assignMusic: async (mood, energy) => {
    await api.musicAssign(mood, energy);
  },

  assignSFX: async (role, positionMs) => {
    await api.sfxAssign(role, positionMs);
  },
}));
```

- [ ] **Step 2: Add asset API calls to tauri.ts**

```typescript
// Add to crates/openscript-tauri/src/frontend/src/lib/tauri.ts

// Asset commands
export async function brollFetch(concepts: string[], download = false) {
  return invoke("broll_fetch", { concepts, download });
}

export async function brollAssign(concept: string, positionMs: number, durationMs: number) {
  return invoke("broll_assign", { concept, positionMs, durationMs });
}

export async function musicSearch(mood?: string, energy?: string) {
  return invoke("music_search", { mood: mood || null, energy: energy || null });
}

export async function musicAssign(mood: string, energy: string) {
  return invoke("music_assign", { mood, energy });
}

export async function sfxSearch(query?: string, role?: string) {
  return invoke("sfx_search", { query: query || "", editorialRole: role || null });
}

export async function sfxAssign(role: string, positionMs: number) {
  return invoke("sfx_assign", { editorialRole: role, positionMs });
}
```

- [ ] **Step 3: Create AssetBrowser with tabs**

```typescript
// crates/openscript-tauri/src/frontend/src/components/assets/AssetBrowser.tsx
import { useState } from "react";
import { cn } from "../../lib/utils";
import { BrollGrid } from "./BrollGrid";
import { MusicList } from "./MusicList";
import { SFXList } from "./SFXList";

type AssetTab = "broll" | "music" | "sfx";

export function AssetBrowser() {
  const [activeTab, setActiveTab] = useState<AssetTab>("broll");

  const tabs: { id: AssetTab; label: string }[] = [
    { id: "broll", label: "B-Roll" },
    { id: "music", label: "Music" },
    { id: "sfx", label: "SFX" },
  ];

  return (
    <div className="flex flex-col h-full bg-background">
      {/* Tabs */}
      <div className="flex border-b">
        {tabs.map((tab) => (
          <button
            key={tab.id}
            className={cn(
              "flex-1 px-3 py-2 text-xs font-medium transition-colors",
              activeTab === tab.id
                ? "text-foreground border-b-2 border-primary"
                : "text-muted-foreground hover:text-foreground"
            )}
            onClick={() => setActiveTab(tab.id)}
          >
            {tab.label}
          </button>
        ))}
      </div>

      {/* Content */}
      <div className="flex-1 overflow-y-auto">
        {activeTab === "broll" && <BrollGrid />}
        {activeTab === "music" && <MusicList />}
        {activeTab === "sfx" && <SFXList />}
      </div>
    </div>
  );
}
```

- [ ] **Step 4: Create BrollGrid**

```typescript
// crates/openscript-tauri/src/frontend/src/components/assets/BrollGrid.tsx
import { useState } from "react";
import { useAssetStore } from "../../store/assets";
import { Search } from "lucide-react";

export function BrollGrid() {
  const [query, setQuery] = useState("");
  const { searchBroll, brollResults, isSearching } = useAssetStore();

  const handleSearch = () => {
    if (!query.trim()) return;
    searchBroll(query.split(",").map((c) => c.trim()));
  };

  return (
    <div className="p-3">
      <div className="flex gap-2 mb-3">
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search concepts (comma-separated)"
          className="flex-1 rounded-md border px-3 py-1.5 text-xs"
          onKeyDown={(e) => e.key === "Enter" && handleSearch()}
        />
        <button
          onClick={handleSearch}
          className="rounded-md bg-primary px-3 py-1.5 text-xs text-primary-foreground"
        >
          <Search className="w-3 h-3" />
        </button>
      </div>

      {isSearching && (
        <div className="text-center text-xs text-muted-foreground py-8">
          Searching Pexels...
        </div>
      )}

      <div className="grid grid-cols-2 gap-2">
        {brollResults.map((result, i) => (
          <div key={i} className="rounded-md overflow-hidden border">
            <div className="aspect-[9/16] bg-muted flex items-center justify-center">
              <span className="text-xs text-muted-foreground">{result.concept}</span>
            </div>
            <div className="px-2 py-1 text-xs">
              {result.videos.length} results
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
```

- [ ] **Step 5: Create MusicList**

```typescript
// crates/openscript-tauri/src/frontend/src/components/assets/MusicList.tsx
import { useEffect, useState } from "react";
import { useAssetStore } from "../../store/assets";

const MOODS = ["neutral", "energetic", "calm", "dramatic", "uplifting"];
const ENERGIES = ["low", "medium", "high"];

export function MusicList() {
  const { searchMusic, musicResults } = useAssetStore();
  const [mood, setMood] = useState("neutral");
  const [energy, setEnergy] = useState("medium");

  useEffect(() => {
    searchMusic(mood, energy);
  }, [mood, energy, searchMusic]);

  return (
    <div className="p-3">
      <div className="flex gap-2 mb-3">
        <select
          value={mood}
          onChange={(e) => setMood(e.target.value)}
          className="flex-1 rounded-md border px-2 py-1 text-xs"
        >
          {MOODS.map((m) => (
            <option key={m} value={m}>{m}</option>
          ))}
        </select>
        <select
          value={energy}
          onChange={(e) => setEnergy(e.target.value)}
          className="flex-1 rounded-md border px-2 py-1 text-xs"
        >
          {ENERGIES.map((e) => (
            <option key={e} value={e}>{e}</option>
          ))}
        </select>
      </div>

      <div className="space-y-1">
        {musicResults.map((track, i) => (
          <div key={i} className="flex items-center justify-between rounded-md border px-3 py-2 text-xs">
            <div>
              <div className="font-medium">{track.title}</div>
              <div className="text-muted-foreground">{track.artist}</div>
            </div>
            <div className="text-right text-muted-foreground">
              <div>{track.mood}</div>
              <div>{Math.round(track.duration_ms / 1000)}s</div>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
```

- [ ] **Step 6: Create SFXList**

```typescript
// crates/openscript-tauri/src/frontend/src/components/assets/SFXList.tsx
import { useEffect, useState } from "react";
import { useAssetStore } from "../../store/assets";

const ROLES = ["intro", "transition", "highlight", "outro"];

export function SFXList() {
  const { searchSFX, sfxResults } = useAssetStore();
  const [query, setQuery] = useState("");

  useEffect(() => {
    searchSFX(query || undefined);
  }, [query, searchSFX]);

  return (
    <div className="p-3">
      <input
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        placeholder="Search SFX..."
        className="w-full rounded-md border px-3 py-1.5 text-xs mb-3"
      />

      <div className="flex gap-1 mb-3 flex-wrap">
        {ROLES.map((role) => (
          <button
            key={role}
            onClick={() => searchSFX(undefined, role)}
            className="rounded-full bg-secondary px-2 py-0.5 text-[10px] text-secondary-foreground hover:bg-secondary/80"
          >
            {role}
          </button>
        ))}
      </div>

      <div className="space-y-1">
        {sfxResults.map((sfx) => (
          <div key={sfx.id} className="flex items-center justify-between rounded-md border px-3 py-2 text-xs">
            <div>
              <div className="font-medium">{sfx.filename}</div>
              <div className="text-muted-foreground">{sfx.category} · {sfx.editorial_role}</div>
            </div>
            <div className="text-muted-foreground">{Math.round(sfx.duration_ms / 1000)}s</div>
          </div>
        ))}
      </div>
    </div>
  );
}
```

- [ ] **Step 7: Commit**

```bash
git add crates/openscript-tauri/src/frontend/src/components/assets/ crates/openscript-tauri/src/frontend/src/store/assets.ts crates/openscript-tauri/src/frontend/src/lib/tauri.ts
git commit -m "feat: asset browser with B-Roll, Music, SFX tabs and search"
```

---

## Phase 6: AI Integration

### Task 6.1: AI Assistant Chat + Reelize Pipeline

**Files:**
- Create: `crates/openscript-tauri/src/frontend/src/store/ai.ts`
- Create: `crates/openscript-tauri/src/frontend/src/components/ai/AIAssistant.tsx`
- Create: `crates/openscript-tauri/src/frontend/src/components/ai/ReelizeButton.tsx`

- [ ] **Step 1: Create AI store**

```typescript
// crates/openscript-tauri/src/frontend/src/store/ai.ts
import { create } from "zustand";
import * as api from "../../lib/tauri";

export interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  content: string;
  timestamp: number;
}

export interface AIState {
  messages: ChatMessage[];
  isProcessing: boolean;
  suggestions: string[];

  sendMessage: (content: string) => Promise<void>;
  runReelize: (videoPath: string) => Promise<void>;
  clear: () => void;
}

export const useAIStore = create<AIState>((set, get) => ({
  messages: [],
  isProcessing: false,
  suggestions: [
    "Create a 30s reel from this video",
    "Add b-roll every 5 seconds",
    "Suggest background music",
    "Generate intro voiceover",
  ],

  sendMessage: async (content: string) => {
    const userMsg: ChatMessage = {
      id: Date.now().toString(),
      role: "user",
      content,
      timestamp: Date.now(),
    };
    set({ messages: [...get().messages, userMsg], isProcessing: true });

    // TODO: Connect to AI agent (local LLM or API)
    // For now, simulate a response
    await new Promise((r) => setTimeout(r, 1000));

    const assistantMsg: ChatMessage = {
      id: (Date.now() + 1).toString(),
      role: "assistant",
      content: `I received your request: "${content}". AI agent integration is coming soon.`,
      timestamp: Date.now(),
    };
    set({ messages: [...get().messages, assistantMsg], isProcessing: false });
  },

  runReelize: async (videoPath: string) => {
    set({ isProcessing: true });
    try {
      // This is the "magic button" — one-call pipeline
      const result = await api.reelizeTimeline(videoPath);
      const msg: ChatMessage = {
        id: Date.now().toString(),
        role: "assistant",
        content: `Reel created! Output: ${(result as any).output_path}`,
        timestamp: Date.now(),
      };
      set({ messages: [...get().messages, msg], isProcessing: false });
    } catch (e) {
      set({ isProcessing: false });
    }
  },

  clear: () => set({ messages: [] }),
}));
```

- [ ] **Step 2: Add reelize commands to tauri.ts and Rust**

```typescript
// Add to tauri.ts
export async function reelizeTimeline(videoPath: string) {
  return invoke("reelize_timeline", { videoPath });
}
```

```rust
// Add to crates/openscript-tauri/src/commands/system.rs
/// One-call pipeline: raw video → complete 9:16 reel.
#[tauri::command(async)]
pub async fn reelize_timeline(
    video_path: String,
    preset: Option<String>,
) -> Result<Value, String> {
    let preset = preset.unwrap_or_else(|| "Balanced".to_string());
    // Call openscript_mcp's reelize.timeline handler
    // This wraps the existing MCP tool as a Tauri command
    Ok(json!({
        "status": "started",
        "video_path": video_path,
        "preset": preset,
        "note": "Reelize pipeline — async rendering in progress"
    }))
}
```

- [ ] **Step 3: Create AIAssistant component**

```typescript
// crates/openscript-tauri/src/frontend/src/components/ai/AIAssistant.tsx
import { useState } from "react";
import { useAIStore } from "../../store/ai";
import { Send, Sparkles } from "lucide-react";
import { cn } from "../../lib/utils";

export function AIAssistant() {
  const { messages, sendMessage, suggestions, isProcessing } = useAIStore();
  const [input, setInput] = useState("");

  const handleSend = () => {
    if (!input.trim() || isProcessing) return;
    sendMessage(input.trim());
    setInput("");
  };

  return (
    <div className="flex flex-col h-full bg-background">
      {/* Header */}
      <div className="px-3 py-2 border-b flex items-center gap-2">
        <Sparkles className="w-4 h-4 text-primary" />
        <span className="text-sm font-medium">AI Assistant</span>
      </div>

      {/* Suggestions */}
      {messages.length === 0 && (
        <div className="p-3">
          <p className="text-xs text-muted-foreground mb-2">Quick actions:</p>
          <div className="space-y-1">
            {suggestions.map((s, i) => (
              <button
                key={i}
                onClick={() => sendMessage(s)}
                className="w-full text-left rounded-md border px-3 py-2 text-xs hover:bg-secondary"
              >
                {s}
              </button>
            ))}
          </div>
        </div>
      )}

      {/* Messages */}
      <div className="flex-1 overflow-y-auto p-3 space-y-3">
        {messages.map((msg) => (
          <div
            key={msg.id}
            className={cn(
              "max-w-[85%] rounded-lg px-3 py-2 text-xs",
              msg.role === "user"
                ? "bg-primary text-primary-foreground ml-auto"
                : "bg-secondary text-secondary-foreground"
            )}
          >
            {msg.content}
          </div>
        ))}
        {isProcessing && (
          <div className="text-xs text-muted-foreground animate-pulse">
            Thinking...
          </div>
        )}
      </div>

      {/* Input */}
      <div className="p-3 border-t">
        <div className="flex gap-2">
          <input
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && handleSend()}
            placeholder="Tell the AI what to do..."
            className="flex-1 rounded-md border px-3 py-2 text-xs"
            disabled={isProcessing}
          />
          <button
            onClick={handleSend}
            disabled={isProcessing || !input.trim()}
            className="rounded-md bg-primary px-3 py-2 text-primary-foreground disabled:opacity-50"
          >
            <Send className="w-3 h-3" />
          </button>
        </div>
      </div>
    </div>
  );
}
```

- [ ] **Step 4: Commit**

```bash
git add crates/openscript-tauri/src/frontend/src/components/ai/ crates/openscript-tauri/src/frontend/src/store/ai.ts crates/openscript-tauri/src/commands/system.rs
git commit -m "feat: AI assistant chat UI + reelize pipeline trigger"
```

---

## Phase 7: Integration Polish

### Task 7.1: Full App Layout + Keyboard Shortcuts

**Files:**
- Modify: `crates/openscript-tauri/src/frontend/src/App.tsx`
- Create: `crates/openscript-tauri/src/frontend/src/hooks/useKeyboardShortcuts.ts`

- [ ] **Step 1: Final App.tsx — complete 4-panel layout**

```typescript
// crates/openscript-tauri/src/frontend/src/App.tsx
import { open } from "@tauri-apps/plugin-dialog";
import { useProjectStore } from "./store/project";
import { useEditorStore } from "./store/editor";
import { TranscriptEditor } from "./components/transcript/TranscriptEditor";
import { TimelineEditor } from "./components/timeline/TimelineEditor";
import { AssetBrowser } from "./components/assets/AssetBrowser";
import { AIAssistant } from "./components/ai/AIAssistant";
import { useKeyboardShortcuts } from "./hooks/useKeyboardShortcuts";
import { cn } from "./lib/utils";
import { FileVideo, Undo2, Redo2, Play, Pause, Save } from "lucide-react";

function TopBar() {
  const { projectName, sourceVideo, createProject, undo, redo, save } = useProjectStore();
  const { isPlaying, setIsPlaying } = useEditorStore();

  const handleOpenVideo = async () => {
    const selected = await open({
      multiple: false,
      filters: [{ name: "Video", extensions: ["mp4", "mov", "avi", "mkv", "webm"] }],
    });
    if (selected && typeof selected === "string") {
      await createProject(selected);
    }
  };

  return (
    <header className="flex h-10 items-center justify-between border-b bg-background px-3">
      <div className="flex items-center gap-3">
        <h1 className="text-sm font-semibold">OpenScript</h1>
        {sourceVideo && (
          <span className="text-xs text-muted-foreground truncate max-w-[200px]">
            {projectName}
          </span>
        )}
      </div>

      <div className="flex items-center gap-1">
        <button onClick={undo} className="p-1.5 rounded hover:bg-secondary" title="Undo (Ctrl+Z)">
          <Undo2 className="w-3.5 h-3.5" />
        </button>
        <button onClick={redo} className="p-1.5 rounded hover:bg-secondary" title="Redo (Ctrl+Shift+Z)">
          <Redo2 className="w-3.5 h-3.5" />
        </button>
        <div className="w-px h-4 bg-border mx-1" />
        <button
          onClick={() => setIsPlaying(!isPlaying)}
          className="p-1.5 rounded hover:bg-secondary"
          title={isPlaying ? "Pause (Space)" : "Play (Space)"}
        >
          {isPlaying ? <Pause className="w-3.5 h-3.5" /> : <Play className="w-3.5 h-3.5" />}
        </button>
        <button onClick={save} className="p-1.5 rounded hover:bg-secondary" title="Save (Ctrl+S)">
          <Save className="w-3.5 h-3.5" />
        </button>
      </div>

      <button
        onClick={handleOpenVideo}
        className="flex items-center gap-1.5 rounded-md bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90"
      >
        <FileVideo className="w-3.5 h-3.5" />
        {sourceVideo ? "Open" : "Open Video"}
      </button>
    </header>
  );
}

function App() {
  const { sourceVideo, error } = useProjectStore();
  const { activePanel, setActivePanel } = useEditorStore();
  useKeyboardShortcuts();

  const panels = [
    { id: "transcript" as const, label: "Transcript" },
    { id: "timeline" as const, label: "Timeline" },
    { id: "assets" as const, label: "Assets" },
    { id: "ai" as const, label: "AI" },
  ];

  if (!sourceVideo) {
    return (
      <div className="flex h-screen w-screen flex-col bg-background text-foreground">
        <TopBar />
        <div className="flex flex-1 items-center justify-center">
          <div className="text-center">
            <FileVideo className="w-12 h-12 mx-auto mb-4 text-muted-foreground" />
            <h2 className="text-xl font-semibold mb-2">Welcome to OpenScript</h2>
            <p className="text-muted-foreground mb-4">Open a video file to start editing</p>
            <button
              onClick={async () => {
                const selected = await open({
                  multiple: false,
                  filters: [{ name: "Video", extensions: ["mp4", "mov", "avi"] }],
                });
                if (selected && typeof selected === "string") {
                  await useProjectStore.getState().createProject(selected);
                }
              }}
              className="rounded-md bg-primary px-4 py-2 text-sm font-medium text-primary-foreground"
            >
              Choose Video
            </button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="flex h-screen w-screen flex-col bg-background text-foreground">
      <TopBar />

      {error && (
        <div className="mx-4 mt-1 rounded-md bg-destructive/10 px-3 py-1.5 text-xs text-destructive">
          {error}
        </div>
      )}

      {/* Main content */}
      <div className="flex flex-1 overflow-hidden">
        {/* Left panel */}
        <div className="w-80 border-r flex flex-col">
          {activePanel === "transcript" && <TranscriptEditor />}
          {activePanel === "ai" && <AIAssistant />}
        </div>

        {/* Center: Video + Timeline */}
        <div className="flex-1 flex flex-col">
          {/* Video preview area */}
          <div className="flex-1 bg-black/5 flex items-center justify-center">
            {/* TODO: <video> element with sourceVideo path */}
            <div className="aspect-[9/16] h-[80%] bg-black rounded-lg flex items-center justify-center">
              <span className="text-muted-foreground text-sm">Video Preview</span>
            </div>
          </div>

          {/* Timeline */}
          <div className="h-48 border-t">
            <TimelineEditor />
          </div>
        </div>

        {/* Right panel */}
        <div className="w-72 border-l flex flex-col">
          {activePanel === "assets" && <AssetBrowser />}
        </div>
      </div>

      {/* Bottom panel tabs */}
      <div className="flex border-t bg-background">
        {panels.map((panel) => (
          <button
            key={panel.id}
            onClick={() => setActivePanel(panel.id)}
            className={cn(
              "flex-1 px-3 py-2 text-xs font-medium transition-colors",
              activePanel === panel.id
                ? "text-foreground border-t-2 border-primary"
                : "text-muted-foreground hover:text-foreground"
            )}
          >
            {panel.label}
          </button>
        ))}
      </div>
    </div>
  );
}

export default App;
```

- [ ] **Step 2: Create keyboard shortcuts hook**

```typescript
// crates/openscript-tauri/src/frontend/src/hooks/useKeyboardShortcuts.ts
import { useEffect } from "react";
import { useProjectStore } from "../store/project";
import { useEditorStore } from "../store/editor";

export function useKeyboardShortcuts() {
  const { undo, redo, save } = useProjectStore();
  const { setIsPlaying, isPlaying } = useEditorStore();

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      // Space: play/pause
      if (e.code === "Space" && e.target === document.body) {
        e.preventDefault();
        setIsPlaying(!isPlaying);
      }

      // Ctrl+Z: undo
      if (e.ctrlKey && e.key === "z" && !e.shiftKey) {
        e.preventDefault();
        undo();
      }

      // Ctrl+Shift+Z or Ctrl+Y: redo
      if ((e.ctrlKey && e.shiftKey && e.key === "z") || (e.ctrlKey && e.key === "y")) {
        e.preventDefault();
        redo();
      }

      // Ctrl+S: save
      if (e.ctrlKey && e.key === "s") {
        e.preventDefault();
        save();
      }

      // Delete: remove selected segment
      if (e.key === "Delete") {
        // TODO: delete selected segment
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [undo, redo, save, setIsPlaying, isPlaying]);
}
```

- [ ] **Step 3: Commit**

```bash
git add crates/openscript-tauri/src/frontend/src/App.tsx crates/openscript-tauri/src/frontend/src/hooks/useKeyboardShortcuts.ts
git commit -m "feat: complete 4-panel app layout + keyboard shortcuts"
```

---

## Self-Review

### Spec Coverage Check

| Requirement | Task | Status |
|-------------|------|--------|
| Tauri scaffolding | Task 1.1, 1.2 | ✅ |
| AppState + Undo/Redo | Task 1.3 | ✅ |
| 5 core commands wired | Task 1.4 | ✅ |
| Frontend invoke layer + store | Task 1.5 | ✅ |
| `system.capabilities` | Task 2.1 | ✅ |
| Speaker/filler detection | Task 2.2 | ✅ |
| Split segment + render fix | Task 2.3 | ✅ |
| Transcript commands | Task 3.1 | ✅ |
| TipTap transcript editor | Task 3.2 | ✅ |
| Multi-track timeline UI | Task 4.1 | ✅ |
| Asset browser (B-Roll, Music, SFX) | Task 5.1 | ✅ |
| AI assistant chat | Task 6.1 | ✅ |
| Keyboard shortcuts | Task 7.1 | ✅ |
| Collaboration features | — | ✅ Explicitly excluded per spec |

### Placeholder Scan
- `reelize_timeline` in system.rs returns a stub response — noted as async placeholder
- Video preview in App.tsx is a placeholder div — marked with TODO
- AI assistant simulates response — marked with TODO
- These are intentional — they depend on backend infrastructure (async rendering, LLM integration, video streaming)

### Type Consistency
- `Segment` type defined in `project.ts` store, used consistently in timeline components
- All Tauri invoke wrappers match Rust command signatures
- `AppState` project/timeline types align with Rust `Timeline` schema

---

## Execution Handoff

Plan complete and saved. Two execution options:

**1. Subagent-Driven (recommended)** — Dispatch a fresh subagent per task (7 phases, 21 tasks), review between tasks, fast parallel execution.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

**Which approach?**