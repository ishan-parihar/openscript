import { useState } from "react";
import { Search } from "lucide-react";
import { useAssetStore } from "../../store/assets";

export function BrollGrid() {
  const [concepts, setConcepts] = useState("");
  const { brollResults, isSearching, searchBroll } = useAssetStore();

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = concepts.trim();
    if (!trimmed) return;
    const list = trimmed.split(",").map((c) => c.trim()).filter(Boolean);
    searchBroll(list, true);
  };

  if (isSearching) {
    return (
      <div className="flex items-center justify-center p-6">
        <p className="text-sm text-muted-foreground">Searching Pexels...</p>
      </div>
    );
  }

  return (
    <div className="p-3">
      <form onSubmit={handleSubmit} className="mb-3 flex gap-2">
        <input
          type="text"
          value={concepts}
          onChange={(e) => setConcepts(e.target.value)}
          placeholder="nature, city, technology"
          className="flex-1 rounded-md border bg-background px-2 py-1.5 text-xs focus:outline-none focus:ring-1 focus:ring-primary"
        />
        <button
          type="submit"
          className="rounded-md bg-primary px-2.5 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90"
        >
          <Search className="h-3.5 w-3.5" />
        </button>
      </form>

      {brollResults.length === 0 ? (
        <p className="text-center text-xs text-muted-foreground">
          Enter concepts to search for b-roll footage
        </p>
      ) : (
        <div className="grid grid-cols-2 gap-2">
          {brollResults.map((result) => (
            <div
              key={result.concept}
              className="overflow-hidden rounded-md border bg-secondary/30"
            >
              <div
                className="flex aspect-[9/16] items-center justify-center bg-muted text-xs text-muted-foreground"
              >
                {result.cached_path ? "Preview" : "No download"}
              </div>
              <div className="p-2">
                <p className="text-xs font-medium capitalize">{result.concept}</p>
                <p className="text-[10px] text-muted-foreground">
                  {result.videos.length} result{result.videos.length !== 1 ? "s" : ""}
                </p>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
