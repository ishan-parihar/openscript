import { Segment } from "../../store/project";
import { SegmentBlock } from "./SegmentBlock";

const TRACK_COLORS: Record<string, string> = {
  dialogue: "#3b82f6",
  voiceover: "#8b5cf6",
  captions: "#eab308",
  "b-roll": "#22c55e",
  music: "#ec4899",
  sfx: "#f97316",
};

interface TrackRowProps {
  trackName: string;
  trackKey: string;
  segments: Segment[];
  zoom: number;
}

export function TrackRow({ trackName, trackKey, segments, zoom }: TrackRowProps) {
  const color = TRACK_COLORS[trackKey] ?? "#6b7280";

  return (
    <div className="flex min-h-[32px] items-center border-b border-white/10">
      <div className="flex w-24 shrink-0 items-center gap-2 px-3">
        <span className="h-2 w-2 shrink-0 rounded-full" style={{ backgroundColor: color }} />
        <span className="truncate text-xs text-white/80">{trackName}</span>
      </div>
      <div className="relative h-8 flex-1" style={{ minWidth: `${zoom * 10}px` }}>
        {trackKey === "dialogue" &&
          segments.map((seg) => (
            <SegmentBlock
              key={seg.id}
              id={seg.id}
              sourceStartMs={seg.source_start_ms}
              sourceEndMs={seg.source_end_ms}
              caption={seg.caption}
              color={color}
            />
          ))}
      </div>
    </div>
  );
}
