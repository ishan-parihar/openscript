import { useEditorStore } from "../../store/editor";

interface SegmentBlockProps {
  id: string;
  sourceStartMs: number;
  sourceEndMs: number;
  caption: string;
  color: string;
}

export function SegmentBlock({ id, sourceStartMs, sourceEndMs, caption, color }: SegmentBlockProps) {
  const { zoom, setSelectedSegmentId } = useEditorStore();

  const left = (sourceStartMs / 1000) * zoom;
  const width = ((sourceEndMs - sourceStartMs) / 1000) * zoom;

  const displayText = caption.length > 40 ? caption.slice(0, 40) + "…" : caption;

  return (
    <div
      className="absolute top-1 flex h-6 cursor-pointer items-center overflow-hidden rounded px-1.5 text-xs text-white transition-opacity hover:opacity-90"
      style={{ left: `${left}px`, width: `${Math.max(width, 4)}px`, backgroundColor: color }}
      onClick={() => setSelectedSegmentId(id)}
      title={caption}
    >
      {width > 30 && <span className="truncate">{displayText}</span>}
    </div>
  );
}
