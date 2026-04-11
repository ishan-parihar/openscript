import { useAIStore } from "../../store/ai";
import { Sparkles } from "lucide-react";

export function AISuggestions() {
  const { suggestions, sendMessage, isProcessing } = useAIStore();

  if (suggestions.length === 0 || isProcessing) return null;

  return (
    <div className="flex flex-wrap gap-2 px-4 py-2">
      {suggestions.map((suggestion) => (
        <button
          key={suggestion}
          onClick={() => sendMessage(suggestion)}
          className="flex items-center gap-1.5 rounded-full border border-primary/30 bg-primary/5 px-3 py-1.5 text-xs text-primary transition-colors hover:border-primary/60 hover:bg-primary/10"
        >
          <Sparkles className="h-3 w-3" />
          {suggestion}
        </button>
      ))}
    </div>
  );
}
