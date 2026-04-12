import { Mic, Wand2, Music, Film, MonitorPlay, Sparkles } from "lucide-react";
import { useProjectStore } from "../../store/project";
import { useTranscriptStore } from "../../store/transcript";

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
    label: "Generate TTS",
    icon: Wand2,
    tooltip: "Generate voiceover from text",
  },
  {
    id: "music",
    label: "Add Music",
    icon: Music,
    tooltip: "Search and assign background music",
  },
  {
    id: "broll",
    label: "Add B-Roll",
    icon: Film,
    tooltip: "Fetch and assign b-roll footage",
  },
  {
    id: "render",
    label: "Render",
    icon: MonitorPlay,
    tooltip: "Render final video",
  },
];

export function ActionToolbar() {
  const { sourceVideo } = useProjectStore();
  const { isTranscribing } = useTranscriptStore();

  return (
    <div className="flex items-center gap-2 border-b bg-background px-4 py-2 shrink-0">
      {ACTIONS.map((action) => {
        const Icon = action.icon;
        const disabled = !sourceVideo || isTranscribing;
        return (
          <button
            key={action.id}
            disabled={disabled}
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
