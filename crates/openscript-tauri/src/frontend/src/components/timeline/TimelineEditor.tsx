import { useRef, useState } from "react";
import { ZoomIn, ZoomOut } from "lucide-react";
import { useEditorStore } from "../../store/editor";
import { useProjectStore } from "../../store/project";
import { TimeRuler } from "./TimeRuler";
import { TrackRow } from "./TrackRow";
import { Playhead } from "./Playhead";

const TRACKS = [
  { key: "dialogue", name: "Dialogue" },
  { key: "voiceover", name: "Voiceover" },
  { key: "captions", name: "Captions" },
  { key: "b-roll", name: "B-Roll" },
  { key: "music", name: "Music" },
  { key: "sfx", name: "SFX" },
  { key: "stickers", name: "Stickers" },
] as const;

export function TimelineEditor() {
  const { zoom, setZoom, playbackPosition, setPlaybackPosition } = useEditorStore();
  const { segments } = useProjectStore();
  const scrollRef = useRef<HTMLDivElement>(null);
  const [totalDurationMs] = useState(60000);

  const handleRulerClick = (ms: number) => {
    setPlaybackPosition(ms);
  };

  const contentHeight = TRACKS.length * 32;

  return (
    <div className="flex h-full flex-col bg-[#1a1a2e] text-white">
      <div className="flex items-center justify-between border-b border-white/10 px-3 py-2">
        <h3 className="text-sm font-medium">Timeline</h3>
        <div className="flex items-center gap-2">
          <button
            onClick={() => setZoom(Math.max(20, zoom - 20))}
            className="rounded p-1 hover:bg-white/10"
          >
            <ZoomOut className="h-4 w-4" />
          </button>
          <input
            type="range"
            min={20}
            max={500}
            value={zoom}
            onChange={(e) => setZoom(Number(e.target.value))}
            className="w-24 accent-blue-500"
          />
          <button
            onClick={() => setZoom(Math.min(500, zoom + 20))}
            className="rounded p-1 hover:bg-white/10"
          >
            <ZoomIn className="h-4 w-4" />
          </button>
          <span className="text-xs text-white/60">{zoom}px/s</span>
        </div>
      </div>

      <div ref={scrollRef} className="flex-1 overflow-x-auto overflow-y-auto">
        <div className="relative" style={{ minWidth: `${Math.max(totalDurationMs / 1000 * zoom, 800)}px` }}>
          <TimeRuler zoom={zoom} durationMs={totalDurationMs} onClick={handleRulerClick} />
          <div className="relative">
            <Playhead positionMs={playbackPosition} zoom={zoom} height={contentHeight} />
            {TRACKS.map((track) => (
              <TrackRow
                key={track.key}
                trackName={track.name}
                trackKey={track.key}
                segments={segments}
                zoom={zoom}
              />
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}
