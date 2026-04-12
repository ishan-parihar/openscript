import { Mic, Wand2, Music, Film, MonitorPlay, Sparkles } from "lucide-react";
import { useProjectStore } from "../../store/project";
import { useTranscriptStore } from "../../store/transcript";
import { useEditorStore } from "../../store/editor";

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
    id: "music",
    label: "Assets",
    icon: Music,
    tooltip: "Browse assets (music, b-roll, SFX)",
  },
  {
    id: "broll",
    label: "Assets",
    icon: Film,
    tooltip: "Browse assets (music, b-roll, SFX)",
  },
  {
    id: "render",
    label: "Render",
    icon: MonitorPlay,
    tooltip: "Open render panel",
  },
];

export function ActionToolbar() {
  const { sourceVideo } = useProjectStore();
  const { isTranscribing, transcribe, analyzeFillerWords, phraseSrtPath } = useTranscriptStore();
  const { setActivePanel } = useEditorStore();

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
      case "music":
      case "broll":
        setActivePanel("assets");
        break;
      case "render":
        setActivePanel("render");
        break;
    }
  };

  return (
    <div className="flex items-center gap-2 border-b bg-background px-4 py-2 shrink-0">
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
    </div>
  );
}
