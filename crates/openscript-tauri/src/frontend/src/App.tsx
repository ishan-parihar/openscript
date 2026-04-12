import { open } from "@tauri-apps/plugin-dialog";
import { Undo2, Redo2, Save } from "lucide-react";
import { useProjectStore } from "./store/project";
import { useEditorStore } from "./store/editor";
import { useKeyboardShortcuts } from "./hooks/useKeyboardShortcuts";
import { VideoViewport } from "./components/video/VideoViewport";
import { PlaybackControls } from "./components/video/PlaybackControls";
import { ActionToolbar } from "./components/toolbar/ActionToolbar";
import { TranscriptEditor } from "./components/transcript/TranscriptEditor";
import { TimelineEditor } from "./components/timeline/TimelineEditor";
import { AssetBrowser } from "./components/assets/AssetBrowser";
import { AIAssistant } from "./components/ai/AIAssistant";
import { Toast, useToastStore } from "./components/shared/Toast";

import { VoicePanel } from "./components/voice/VoicePanel";
import { RenderPanel } from "./components/render/RenderPanel";

const PANELS: { key: "transcript" | "timeline" | "assets" | "ai" | "voice" | "render"; label: string }[] = [
  { key: "transcript", label: "Transcript" },
  { key: "timeline", label: "Timeline" },
  { key: "assets", label: "Assets" },
  { key: "voice", label: "Voice" },
  { key: "render", label: "Render" },
  { key: "ai", label: "AI" },
];

function TopBar() {
  const { projectName, sourceVideo, createProject, undo, redo, save } = useProjectStore();
  const { toasts, dismissToast } = useToastStore();

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
    <>
      <header className="flex h-10 items-center justify-between border-b bg-background px-4 shrink-0">
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

        <div className="flex items-center gap-1">
          <button
            onClick={() => undo()}
            disabled={!sourceVideo}
            className="rounded-md p-1.5 text-muted-foreground hover:bg-secondary disabled:opacity-30"
            title="Undo"
          >
            <Undo2 className="h-4 w-4" />
          </button>
          <button
            onClick={() => redo()}
            disabled={!sourceVideo}
            className="rounded-md p-1.5 text-muted-foreground hover:bg-secondary disabled:opacity-30"
            title="Redo"
          >
            <Redo2 className="h-4 w-4" />
          </button>
          <button
            onClick={() => save()}
            disabled={!sourceVideo}
            className="rounded-md p-1.5 text-muted-foreground hover:bg-secondary disabled:opacity-30"
            title="Save"
          >
            <Save className="h-4 w-4" />
          </button>
        </div>

        <button
          onClick={handleOpenVideo}
          className="rounded-md bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90"
        >
          {sourceVideo ? "Open Another" : "Open Video"}
        </button>
      </header>
      <Toast toasts={toasts} onDismiss={dismissToast} />
    </>
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

function PanelSwitcher() {
  const { activePanel, setActivePanel } = useEditorStore();

  return (
    <div className="flex items-center justify-center border-b bg-background px-4 shrink-0">
      <div className="flex gap-1">
        {PANELS.map((panel) => (
          <button
            key={panel.key}
            onClick={() => setActivePanel(panel.key)}
            className={`rounded-md px-4 py-1.5 text-xs font-medium transition-colors ${
              activePanel === panel.key
                ? "bg-primary text-primary-foreground"
                : "text-muted-foreground hover:bg-secondary hover:text-foreground"
            }`}
          >
            {panel.label}
          </button>
        ))}
      </div>
    </div>
  );
}

function App() {
  const { sourceVideo, error } = useProjectStore();
  const { activePanel } = useEditorStore();
  useKeyboardShortcuts();

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
        <>
          <ActionToolbar />
          <div className="flex flex-1 overflow-hidden">
            <div className="flex flex-col w-1/2 min-w-[400px] border-r">
              <div className="flex-1 overflow-hidden">
                <VideoViewport />
              </div>
              <PlaybackControls />
            </div>

            <div className="flex flex-col w-1/2 min-w-[400px]">
              <PanelSwitcher />
              <div className="flex-1 overflow-hidden">
                {activePanel === "transcript" && <TranscriptEditor />}
                {activePanel === "timeline" && <TimelineEditor />}
                {activePanel === "assets" && <AssetBrowser />}
                {activePanel === "voice" && <VoicePanel />}
                {activePanel === "render" && <RenderPanel />}
                {activePanel === "ai" && <AIAssistant />}
              </div>
            </div>
          </div>
        </>
      )}
    </div>
  );
}

export default App;
