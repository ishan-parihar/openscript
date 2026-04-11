import { useEffect, useState } from "react";
import { useAssetStore } from "../../store/assets";

const MOODS = ["neutral", "energetic", "calm", "dramatic", "uplifting"];
const ENERGIES = ["low", "medium", "high"];

export function MusicList() {
  const [mood, setMood] = useState("neutral");
  const [energy, setEnergy] = useState("medium");
  const { musicResults, isSearching, searchMusic } = useAssetStore();

  useEffect(() => {
    searchMusic(mood, energy);
  }, [mood, energy]);

  if (isSearching && musicResults.length === 0) {
    return (
      <div className="flex items-center justify-center p-6">
        <p className="text-sm text-muted-foreground">Loading music...</p>
      </div>
    );
  }

  return (
    <div className="p-3">
      <div className="mb-3 flex gap-2">
        <select
          value={mood}
          onChange={(e) => setMood(e.target.value)}
          className="flex-1 rounded-md border bg-background px-2 py-1.5 text-xs focus:outline-none focus:ring-1 focus:ring-primary"
        >
          {MOODS.map((m) => (
            <option key={m} value={m}>{m}</option>
          ))}
        </select>
        <select
          value={energy}
          onChange={(e) => setEnergy(e.target.value)}
          className="flex-1 rounded-md border bg-background px-2 py-1.5 text-xs focus:outline-none focus:ring-1 focus:ring-primary"
        >
          {ENERGIES.map((e) => (
            <option key={e} value={e}>{e}</option>
          ))}
        </select>
      </div>

      {musicResults.length === 0 ? (
        <p className="text-center text-xs text-muted-foreground">
          No tracks found for this combination
        </p>
      ) : (
        <div className="flex flex-col gap-1.5">
          {musicResults.map((track, i) => (
            <div key={i} className="rounded-md border bg-secondary/20 p-2.5">
              <p className="text-xs font-medium">{track.title}</p>
              <p className="text-[10px] text-muted-foreground">{track.artist}</p>
              <div className="mt-1.5 flex items-center gap-2">
                <span className="rounded-full bg-primary/10 px-1.5 py-0.5 text-[10px] capitalize text-primary">
                  {track.mood}
                </span>
                <span className="text-[10px] text-muted-foreground">
                  {Math.floor(track.duration_ms / 1000)}s
                </span>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
