import { cn } from "../../lib/utils";
import { formatTimecode } from "../../lib/utils";

interface TranscriptSegmentProps {
  index: number;
  start: number;
  end: number;
  text: string;
  isActive?: boolean;
  onClick?: () => void;
}

export function TranscriptSegment({
  start,
  end: _end,
  text,
  isActive = false,
  onClick,
}: TranscriptSegmentProps) {
  return (
    <div
      className={cn(
        "group flex cursor-pointer items-start gap-3 rounded-md px-3 py-2 transition-colors",
        isActive
          ? "bg-primary/10 text-foreground"
          : "hover:bg-secondary/50 text-muted-foreground",
      )}
      onClick={onClick}
      role="button"
      tabIndex={0}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onClick?.();
        }
      }}
    >
      <span
        className={cn(
          "shrink-0 font-mono text-xs tabular-nums",
          isActive ? "text-primary" : "text-muted-foreground/60 group-hover:text-muted-foreground",
        )}
      >
        {formatTimecode(start)}
      </span>
      <p className="flex-1 text-sm leading-relaxed">{text}</p>
    </div>
  );
}
