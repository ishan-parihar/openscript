import { useAIStore } from "../../store/ai";
import { useProjectStore } from "../../store/project";
import { Wand2, Loader2 } from "lucide-react";
import { cn } from "../../lib/utils";

export function ReelizeButton() {
  const { sourceVideo } = useProjectStore();
  const { isProcessing, runReelize } = useAIStore();

  const disabled = !sourceVideo || isProcessing;

  const handleClick = () => {
    if (sourceVideo) {
      runReelize(sourceVideo);
    }
  };

  return (
    <button
      onClick={handleClick}
      disabled={disabled}
      className={cn(
        "inline-flex items-center gap-2 rounded-lg px-4 py-2.5 text-sm font-medium text-white transition-all",
        "bg-gradient-to-r from-violet-600 to-blue-600 hover:from-violet-500 hover:to-blue-500",
        "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-violet-500 focus-visible:ring-offset-2",
        "disabled:pointer-events-none disabled:opacity-50",
        "shadow-lg shadow-violet-500/20 hover:shadow-violet-500/30",
      )}
    >
      {isProcessing ? (
        <Loader2 className="h-4 w-4 animate-spin" />
      ) : (
        <Wand2 className="h-4 w-4" />
      )}
      {isProcessing ? "Processing..." : "Reelize"}
    </button>
  );
}
