interface PlayheadProps {
  positionMs: number;
  zoom: number;
  height: number;
}

export function Playhead({ positionMs, zoom, height }: PlayheadProps) {
  const left = (positionMs / 1000) * zoom;

  return (
    <div
      className="absolute z-20"
      style={{ left: `${left}px`, top: 0, height: `${height}px` }}
    >
      <svg width="10" height="10" className="absolute -left-[3px] -top-0.5">
        <polygon points="0,0 10,0 5,8" fill="#ef4444" />
      </svg>
      <div className="w-0.5 bg-red-500" style={{ height: `${height}px` }} />
    </div>
  );
}
