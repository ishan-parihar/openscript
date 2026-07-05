import { useState, useEffect } from "react";
import { Mic, Wand2, Music, MonitorPlay, Sparkles, FileVideo, Activity } from "lucide-react";
import { useProjectStore } from "../../store/project";
import { useTranscriptStore } from "../../store/transcript";
import { useEditorStore } from "../../store/editor";
import { useAIStore } from "../../store/ai";
import * as api from "../../lib/tauri";

const ACTIONS = [
  {
    id: "transcribe",
    label: "Transcribe",
    icon: Mic,
    tooltip: "Transcribe audio to text",
  },
  {
    id: "analyze",
    label: "Analyze",
    icon: Sparkles,
    tooltip: "Analyze transcript for filler words",
  },
  {
    id: "tts",
    label: "Voice",
    icon: Wand2,
    tooltip: "Open voice panel for TTS generation",
  },
  {
    id: "assets",
    label: "Assets",
    icon: Music,
    tooltip: "Browse assets (music, b-roll, SFX)",
  },
  {
    id: "render",
    label: "Render",
    icon: MonitorPlay,
    tooltip: "Open render panel",
  },
];

/** Compact capabilities indicator. Probes system.capabilities on mount and
 *  shows a green/yellow/red dot summarising how many subsystems are wired. */
function CapabilitiesIndicator() {
  const [summary, setSummary] = useState<{ available: number; total: number; detail: string } | null>(null);

  useEffect(() => {
    let cancelled = false;
    void api.systemCapabilities()
      .then((caps) => {
        if (cancelled) return;
        const subs = [
          caps.ffmpeg,
          caps.kokoro,
          caps.transcription,
          caps.voicebox,
          caps.pexels,
          caps.giphy,
          caps.sfx_library,
          caps.music_library,
          caps.hyperframes,
        ];
        const available = subs.filter((s) => s.available).length;
        const total = subs.length;
        const color = available === total ? "bg-green-500" : available >= total / 2 ? "bg-yellow-500" : "bg-red-500";
        setSummary({ available, total, detail: `${available}/${total} subsystems available — click to probe in the command palette` });
        // stash the color on the element via a data attribute hack
        const el = document.getElementById("caps-dot");
        if (el) el.className = `h-2 w-2 rounded-full ${color}`;
      })
      .catch(() => {
        if (cancelled) return;
        setSummary({ available: 0, total: 9, detail: "Probe failed" });
      });
    return () => { cancelled = true; };
  }, []);

  const { probeCapabilities } = useAIStore();
  const { setActivePanel } = useEditorStore();

  return (
    <button
      onClick={() => {
        setActivePanel("ai");
        void probeCapabilities();
      }}
      className="flex items-center gap-1.5 rounded-md border px-2 py-1 text-xs text-muted-foreground hover:bg-secondary"
      title={summary?.detail ?? "Probing subsystems..."}
    >
      <span id="caps-dot" className="h-2 w-2 rounded-full bg-muted-foreground/40" />
      <Activity className="h-3 w-3" />
    </button>
  );
}

export function ActionToolbar() {
  const { sourceVideo } = useProjectStore();
  const { isTranscribing, transcribe, analyzeFillerWords, phraseSrtPath } = useTranscriptStore();
  const { setActivePanel } = useEditorStore();
  const { sendMessage } = useAIStore();

  const handleClick = async (actionId: string) => {
    switch (actionId) {
      case "transcribe":
        if (sourceVideo) {
          await transcribe(sourceVideo);
        }
        break;
      case "analyze":
        if (phraseSrtPath) {
          await analyzeFillerWords(phraseSrtPath);
        }
        break;
      case "tts":
        setActivePanel("voice");
        break;
      case "assets":
        setActivePanel("assets");
        break;
      case "render":
        setActivePanel("render");
        break;
      case "golden":
        // Route to the command palette which will load the script.to_video args form.
        setActivePanel("ai");
        await sendMessage("Create a video from a script");
        break;
    }
  };

  return (
    <div className="flex items-center gap-2 border-b bg-background px-4 py-2 shrink-0">
      {/* Golden trajectory: script.to_video one-call button */}
      <button
        onClick={() => handleClick("golden")}
        className="flex items-center gap-1.5 rounded-md bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90 transition-colors"
        title="One-call: script JSON → finished MP4 (TTS + captions + backgrounds + stickers + music + render)"
      >
        <FileVideo className="h-3.5 w-3.5" />
        Create from Script
      </button>

      <div className="h-4 w-px bg-border" />

      {ACTIONS.map((action) => {
        const Icon = action.icon;
        const isTranscribingDisabled = action.id === "transcribe" && isTranscribing;
        return (
          <button
            key={action.id}
            disabled={!sourceVideo || isTranscribingDisabled}
            onClick={() => handleClick(action.id)}
            className="flex items-center gap-1.5 rounded-md border px-3 py-1.5 text-xs font-medium hover:bg-secondary hover:border-primary/50 disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
            title={action.tooltip}
          >
            <Icon className="h-3.5 w-3.5" />
            {action.label}
          </button>
        );
      })}

      <div className="flex-1" />

      <CapabilitiesIndicator />
    </div>
  );
}
