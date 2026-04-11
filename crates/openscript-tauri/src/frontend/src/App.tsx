import { open } from "@tauri-apps/plugin-dialog";
import { useProjectStore } from "./store/project";


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
          <div className="flex-1 flex items-center justify-center bg-black/5">
            <p className="text-muted-foreground text-sm">
              Video preview — {segments.length} segment(s)
            </p>
          </div>

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
