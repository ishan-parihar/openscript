import { open } from "@tauri-apps/plugin-dialog";
import { useProjectStore } from "./store/project";
import { TranscriptEditor } from "./components/transcript/TranscriptEditor";
import { TimelineEditor } from "./components/timeline/TimelineEditor";

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

function AssetsPanel() {
  return (
    <div className="flex h-full flex-col">
      <div className="border-b p-3">
        <h3 className="text-sm font-medium">Assets</h3>
      </div>
      <div className="flex flex-1 flex-col items-center justify-center gap-4 p-4">
        <div className="flex w-full gap-1">
          {["B-Roll", "Music", "SFX"].map((tab) => (
            <button
              key={tab}
              className="flex-1 rounded-md px-2 py-1.5 text-xs font-medium text-muted-foreground hover:bg-secondary hover:text-foreground"
            >
              {tab}
            </button>
          ))}
        </div>
        <p className="text-center text-xs text-muted-foreground">
          Asset library coming soon
        </p>
      </div>
    </div>
  );
}

function App() {
  const { sourceVideo, error } = useProjectStore();

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
        <div className="flex flex-1 overflow-hidden">
          <div className="w-80 shrink-0 border-r overflow-y-auto">
            <TranscriptEditor />
          </div>

          <div className="flex flex-1 flex-col overflow-hidden">
            <div className="flex h-1/2 items-center justify-center border-b bg-black/5">
              <p className="text-muted-foreground text-sm">
                Video preview
              </p>
            </div>
            <div className="h-1/2 overflow-hidden">
              <TimelineEditor />
            </div>
          </div>

          <div className="w-64 shrink-0 border-l overflow-y-auto">
            <AssetsPanel />
          </div>
        </div>
      )}
    </div>
  );
}

export default App;
