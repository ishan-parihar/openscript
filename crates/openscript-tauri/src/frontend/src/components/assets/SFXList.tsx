import { useState } from "react";
import { useAssetStore } from "../../store/assets";

const ROLES = ["intro", "transition", "highlight", "outro"];

export function SFXList() {
  const [query, setQuery] = useState("");
  const [role, setRole] = useState<string | null>(null);
  const { sfxResults, isSearching, searchSFX } = useAssetStore();

  const handleSearch = (value: string) => {
    setQuery(value);
    searchSFX(value || undefined, role || undefined);
  };

  const handleRoleToggle = (r: string) => {
    const next = role === r ? null : r;
    setRole(next);
    searchSFX(query || undefined, next || undefined);
  };

  if (isSearching && sfxResults.length === 0) {
    return (
      <div className="flex items-center justify-center p-6">
        <p className="text-sm text-muted-foreground">Searching SFX...</p>
      </div>
    );
  }

  return (
    <div className="p-3">
      <input
        type="text"
        value={query}
        onChange={(e) => handleSearch(e.target.value)}
        placeholder="whoosh, click, boom"
        className="mb-2 w-full rounded-md border bg-background px-2 py-1.5 text-xs focus:outline-none focus:ring-1 focus:ring-primary"
      />

      <div className="mb-3 flex flex-wrap gap-1.5">
        {ROLES.map((r) => (
          <button
            key={r}
            onClick={() => handleRoleToggle(r)}
            className={`rounded-full px-2.5 py-0.5 text-[10px] font-medium capitalize transition-colors ${
              role === r
                ? "bg-primary text-primary-foreground"
                : "bg-secondary text-muted-foreground hover:text-foreground"
            }`}
          >
            {r}
          </button>
        ))}
      </div>

      {sfxResults.length === 0 ? (
        <p className="text-center text-xs text-muted-foreground">
          Search for sound effects by name or role
        </p>
      ) : (
        <div className="flex flex-col gap-1.5">
          {sfxResults.map((sfx) => (
            <div key={sfx.id} className="rounded-md border bg-secondary/20 p-2.5">
              <p className="text-xs font-medium">{sfx.filename}</p>
              <div className="mt-1 flex items-center gap-2">
                <span className="text-[10px] text-muted-foreground">
                  {sfx.category}
                </span>
                <span className="rounded-full bg-primary/10 px-1.5 py-0.5 text-[10px] capitalize text-primary">
                  {sfx.editorial_role}
                </span>
                <span className="text-[10px] text-muted-foreground">
                  {Math.floor(sfx.duration_ms / 1000)}s
                </span>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
