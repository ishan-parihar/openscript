import { useEffect } from "react";
import { useProjectStore } from "../store/project";
import { useEditorStore } from "../store/editor";

export function useKeyboardShortcuts() {
  const { undo, redo, save } = useProjectStore();
  const { isPlaying, setIsPlaying, playbackPosition, setPlaybackPosition } = useEditorStore();

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      const isMod = e.ctrlKey || e.metaKey;

      if (isMod && e.key === "z" && !e.shiftKey) {
        e.preventDefault();
        undo();
        return;
      }

      if (isMod && e.key === "Z" && e.shiftKey) {
        e.preventDefault();
        redo();
        return;
      }

      if (isMod && e.key === "s") {
        e.preventDefault();
        save();
        return;
      }

      if (e.key === " " && !isMod) {
        const target = e.target as HTMLElement;
        if (target.tagName === "INPUT" || target.tagName === "TEXTAREA") return;
        e.preventDefault();
        setIsPlaying(!isPlaying);
        return;
      }

      if (e.key === "ArrowLeft" && !isMod) {
        const target = e.target as HTMLElement;
        if (target.tagName === "INPUT" || target.tagName === "TEXTAREA") return;
        e.preventDefault();
        setPlaybackPosition(Math.max(0, playbackPosition - 1000));
        return;
      }

      if (e.key === "ArrowRight" && !isMod) {
        const target = e.target as HTMLElement;
        if (target.tagName === "INPUT" || target.tagName === "TEXTAREA") return;
        e.preventDefault();
        setPlaybackPosition(playbackPosition + 1000);
        return;
      }
    };

    document.addEventListener("keydown", handler);
    return () => document.removeEventListener("keydown", handler);
  }, [undo, redo, save, isPlaying, setIsPlaying, playbackPosition, setPlaybackPosition]);
}
