import { Sparkles, X, Check, Video } from "lucide-react";
import { useRenderStore } from "../../store/render";

export function RenderPanel() {
  const {
    quality,
    includeCaptions,
    includeBroll,
    includeMusic,
    includeSFX,
    isRendering,
    progress,
    status,
    etaSeconds,
    outputPath,
    fileSizeBytes,
    durationMs,
    error,
    setQuality,
    toggleCaptions,
    toggleBroll,
    toggleMusic,
    toggleSFX,
    render,
    cancelRender,
  } = useRenderStore();

  const qualities: { value: "preview" | "standard" | "high"; label: string; desc: string }[] = [
    { value: "preview", label: "Preview", desc: "Fast" },
    { value: "standard", label: "Standard", desc: "Balanced" },
    { value: "high", label: "High", desc: "Best" },
  ];

  const toggles = [
    { label: "Captions", active: includeCaptions, toggle: toggleCaptions },
    { label: "B-Roll", active: includeBroll, toggle: toggleBroll },
    { label: "Music", active: includeMusic, toggle: toggleMusic },
    { label: "SFX", active: includeSFX, toggle: toggleSFX },
  ] as const;

  const formatDuration = (ms: number) => {
    const totalSec = Math.floor(ms / 1000);
    const min = Math.floor(totalSec / 60);
    const sec = totalSec % 60;
    return `${min}:${sec.toString().padStart(2, "0")}`;
  };

  return (
    <div className="flex flex-col gap-4 p-3">
      <div>
        <div className="mb-2 flex items-center gap-1.5 text-xs font-medium text-muted-foreground">
          <Video className="h-3 w-3" />
          Quality
        </div>
        <div className="flex gap-1">
          {qualities.map((q) => (
            <button
              key={q.value}
              onClick={() => setQuality(q.value)}
              disabled={isRendering}
              className={`flex flex-1 flex-col items-center rounded-md px-2 py-1.5 text-xs transition-colors ${
                quality === q.value
                  ? "bg-primary text-primary-foreground"
                  : "bg-secondary text-muted-foreground hover:text-foreground"
              }`}
            >
              <span className="font-medium">{q.label}</span>
              <span className="text-[10px] opacity-70">{q.desc}</span>
            </button>
          ))}
        </div>
      </div>

      <div>
        <div className="mb-2 text-xs font-medium text-muted-foreground">Include</div>
        <div className="grid grid-cols-2 gap-1.5">
          {toggles.map((t) => (
            <button
              key={t.label}
              onClick={t.toggle}
              disabled={isRendering}
              className={`flex items-center gap-1.5 rounded-md border px-2 py-1.5 text-xs transition-colors ${
                t.active
                  ? "border-primary bg-primary/10 text-foreground"
                  : "border-transparent bg-secondary/50 text-muted-foreground"
              }`}
            >
              {t.active && <Check className="h-3 w-3 text-primary" />}
              {t.label}
            </button>
          ))}
        </div>
      </div>

      {isRendering && (
        <div className="flex flex-col gap-1.5">
          <div className="h-2 w-full overflow-hidden rounded-full bg-muted">
            <div
              className="h-full rounded-full bg-primary transition-all duration-300"
              style={{ width: `${progress}%` }}
            />
          </div>
          <div className="text-[11px] text-muted-foreground">
            {status}
            {etaSeconds != null && etaSeconds > 0
              ? ` · ~${Math.ceil(etaSeconds)}s remaining`
              : ""}
          </div>
        </div>
      )}

      {error && (
        <div className="rounded-md bg-destructive/10 px-3 py-2 text-sm text-destructive">
          {error}
        </div>
      )}

      {outputPath && !isRendering && (
        <div className="flex flex-col gap-1.5">
          <div className="text-sm font-medium text-foreground">Render complete!</div>
          <div className="flex items-center gap-2 text-xs text-muted-foreground">
            <span className="truncate max-w-xs">{outputPath}</span>
          </div>
          <div className="flex items-center gap-3 text-[11px] text-muted-foreground">
            {fileSizeBytes != null && (
              <span>
                {fileSizeBytes > 1048576
                  ? `${(fileSizeBytes / 1048576).toFixed(1)} MB`
                  : `${(fileSizeBytes / 1024).toFixed(0)} KB`}
              </span>
            )}
            {durationMs != null && <span>{formatDuration(durationMs)}</span>}
            <span className="text-primary font-medium">Play</span>
          </div>
        </div>
      )}

      {isRendering ? (
        <button
          onClick={cancelRender}
          className="flex w-full items-center justify-center gap-2 rounded-md bg-destructive/90 py-2 text-sm font-medium text-destructive-foreground transition-colors hover:bg-destructive"
        >
          <X className="h-4 w-4" />
          Cancel
        </button>
      ) : (
        <button
          onClick={render}
          className="flex w-full items-center justify-center gap-2 rounded-md bg-primary py-2 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/90"
        >
          <Sparkles className="h-4 w-4" />
          Render
        </button>
      )}
    </div>
  );
}
