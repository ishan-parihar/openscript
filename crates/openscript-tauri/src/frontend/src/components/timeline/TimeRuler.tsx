import { formatTimecode } from "../../lib/utils";

interface TimeRulerProps {
  zoom: number;
  durationMs: number;
  onClick: (ms: number) => void;
}

export function TimeRuler({ zoom, durationMs, onClick }: TimeRulerProps) {
  const durationSec = Math.ceil(durationMs / 1000);
  const intervalSec = zoom > 200 ? 1 : zoom > 80 ? 2 : 5;
  const markers: number[] = [];
  for (let s = 0; s <= durationSec; s += intervalSec) {
    markers.push(s);
  }

  return (
    <div
      className="sticky top-0 z-10 h-8 border-b bg-[#1a1a2e] text-white"
      style={{ width: `${Math.max(durationSec * zoom, 800)}px` }}
    >
      {markers.map((s) => (
        <div
          key={s}
          className="absolute top-0 flex h-full cursor-pointer items-end hover:bg-white/10"
          style={{ left: `${s * zoom}px` }}
          onClick={() => onClick(s * 1000)}
        >
          <div className="h-2 w-px bg-white/50" />
          <span className="ml-1 pb-1 text-[10px] leading-none text-white/70">
            {formatTimecode(s)}
          </span>
        </div>
      ))}
    </div>
  );
}
