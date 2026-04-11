import { useState } from "react";
import { BrollGrid } from "./BrollGrid";
import { MusicList } from "./MusicList";
import { SFXList } from "./SFXList";

type AssetTab = "B-Roll" | "Music" | "SFX";

const TABS: AssetTab[] = ["B-Roll", "Music", "SFX"];

export function AssetBrowser() {
  const [activeTab, setActiveTab] = useState<AssetTab>("B-Roll");

  return (
    <div className="flex h-full flex-col">
      <div className="flex shrink-0 border-b">
        {TABS.map((tab) => (
          <button
            key={tab}
            onClick={() => setActiveTab(tab)}
            className={`flex-1 px-2 py-2 text-xs font-medium transition-colors ${
              activeTab === tab
                ? "border-b-2 border-primary text-foreground"
                : "text-muted-foreground hover:bg-secondary hover:text-foreground"
            }`}
          >
            {tab}
          </button>
        ))}
      </div>
      <div className="flex-1 overflow-y-auto">
        {activeTab === "B-Roll" && <BrollGrid />}
        {activeTab === "Music" && <MusicList />}
        {activeTab === "SFX" && <SFXList />}
      </div>
    </div>
  );
}
