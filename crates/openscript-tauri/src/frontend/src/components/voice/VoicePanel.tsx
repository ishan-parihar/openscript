import { useEffect, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { Mic, User, Loader2, Clock } from "lucide-react";
import { useVoiceStore } from "../../store/voice";

type VoiceTab = "Generate" | "Profiles";

const TABS: VoiceTab[] = ["Generate", "Profiles"];

export function VoicePanel() {
  const [activeTab, setActiveTab] = useState<VoiceTab>("Generate");
  const {
    profiles,
    selectedProfileId,
    text,
    isGenerating,
    generatedAudioPath,
    estimatedDurationMs,
    loadProfiles,
    setText,
    setSelectedProfile,
    generate,
    estimateDuration,
  } = useVoiceStore();

  useEffect(() => {
    loadProfiles();
  }, []);

  return (
    <div className="flex h-full flex-col">
      <div className="flex shrink-0 border-b">
        {TABS.map((tab) => (
          <button
            key={tab}
            onClick={() => setActiveTab(tab)}
            className={`flex items-center justify-center gap-1.5 flex-1 px-2 py-2 text-xs font-medium transition-colors ${
              activeTab === tab
                ? "border-b-2 border-primary text-foreground"
                : "text-muted-foreground hover:bg-secondary hover:text-foreground"
            }`}
          >
            {tab === "Generate" ? <Mic className="h-3.5 w-3.5" /> : <User className="h-3.5 w-3.5" />}
            {tab}
          </button>
        ))}
      </div>

      <div className="flex-1 overflow-y-auto">
        {activeTab === "Generate" && (
          <div className="p-3">
            <div className="mb-3">
              <select
                value={selectedProfileId ?? ""}
                onChange={(e) => setSelectedProfile(e.target.value || null)}
                className="w-full rounded-md border bg-background px-2 py-1.5 text-xs focus:outline-none focus:ring-1 focus:ring-primary"
              >
                <option value="">No voice (default)</option>
                {profiles.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.name}
                  </option>
                ))}
              </select>
            </div>

            <textarea
              value={text}
              onChange={(e) => setText(e.target.value)}
              placeholder="Enter voiceover text..."
              rows={5}
              className="w-full rounded-md border bg-background px-2 py-1.5 text-xs focus:outline-none focus:ring-1 focus:ring-primary"
            />

            <div className="mb-3 flex items-center justify-between">
              <span className="text-[10px] text-muted-foreground">{text.length} chars</span>
              {estimatedDurationMs !== null && (
                <span className="flex items-center gap-1 text-[10px] text-muted-foreground">
                  <Clock className="h-3 w-3" />
                  ~{Math.round(estimatedDurationMs / 1000)}s
                </span>
              )}
            </div>

            <div className="mb-3 flex gap-2">
              <button
                onClick={estimateDuration}
                disabled={!text}
                className="flex-1 rounded-md border bg-background px-3 py-1.5 text-xs font-medium transition-colors hover:bg-secondary disabled:opacity-50"
              >
                Estimate
              </button>
              <button
                onClick={generate}
                disabled={!text || isGenerating}
                className="flex items-center justify-center gap-1.5 flex-1 rounded-md bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90 disabled:opacity-50"
              >
                {isGenerating ? (
                  <>
                    <Loader2 className="h-3 w-3 animate-spin" />
                    Generating...
                  </>
                ) : (
                  "Generate"
                )}
              </button>
            </div>

            {generatedAudioPath && (
              <div className="rounded-md border bg-secondary/20 p-3">
                <audio
                  controls
                  src={convertFileSrc(generatedAudioPath, "http")}
                  className="w-full"
                />
              </div>
            )}
          </div>
        )}

        {activeTab === "Profiles" && (
          <div className="p-3">
            {profiles.length === 0 ? (
              <div className="flex items-center justify-center p-6">
                <p className="text-center text-xs text-muted-foreground">
                  No profiles found. Add profiles via the MCP server or config.
                </p>
              </div>
            ) : (
              <div className="flex flex-col gap-1.5">
                {profiles.map((p) => (
                  <div
                    key={p.id}
                    className="rounded-md border bg-secondary/20 p-2.5"
                  >
                    <p className="text-xs font-medium">{p.name}</p>
                    <p className="text-[10px] text-muted-foreground">{p.language}</p>
                  </div>
                ))}
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
