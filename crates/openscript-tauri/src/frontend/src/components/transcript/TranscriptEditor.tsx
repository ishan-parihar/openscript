import { useState, useCallback, useMemo, useEffect } from "react";
import { useTranscriptStore } from "../../store/transcript";
import { useProjectStore } from "../../store/project";
import { useEditorStore } from "../../store/editor";
import { useToastStore } from "../shared/Toast";
import { isFillerWord } from "./WordToken";
import { Mic, Eraser, Save } from "lucide-react";

interface WordEntry {
  id: string;
  text: string;
  start: number;
  end: number;
  isFiller: boolean;
  segmentIndex: number;
}

export function TranscriptEditor() {
  const { sourceVideo } = useProjectStore();
  const { entries, isTranscribing, transcriptionProgress, wordSrtPath } = useTranscriptStore();
  const { setPlaybackPosition } = useEditorStore();
  const { addToast } = useToastStore();

  const { transcribe, loadTranscript, applyEdit } = useTranscriptStore();

  const [includedWords, setIncludedWords] = useState<Set<string>>(new Set());
  const [isApplying, setIsApplying] = useState(false);

  const wordEntries: WordEntry[] = useMemo(() => {
    const words: WordEntry[] = [];
    for (let segIdx = 0; segIdx < entries.length; segIdx++) {
      const entry = entries[segIdx];
      const entryWords = entry.text.split(/\s+/).filter(Boolean);
      const durationSec = entry.end - entry.start;
      const avgWordDuration = durationSec / entryWords.length;

      for (let i = 0; i < entryWords.length; i++) {
        const wordStart = entry.start + i * avgWordDuration;
        const wordEnd = entry.start + (i + 1) * avgWordDuration;
        const cleanWord = entryWords[i].replace(/[.,!?;:'"()]/g, "");

        words.push({
          id: `w_${segIdx}_${i}`,
          text: entryWords[i],
          start: wordStart,
          end: wordEnd,
          isFiller: isFillerWord(cleanWord),
          segmentIndex: segIdx,
        });
      }
    }
    return words;
  }, [entries]);

  useEffect(() => {
    setIncludedWords(new Set(wordEntries.map((w) => w.id)));
  }, [wordEntries]);

  const toggleWord = useCallback((wordId: string) => {
    setIncludedWords((prev) => {
      const next = new Set(prev);
      if (next.has(wordId)) {
        next.delete(wordId);
      } else {
        next.add(wordId);
      }
      return next;
    });
  }, []);

  const handleWordClick = useCallback(
    (word: WordEntry) => {
      toggleWord(word.id);
      setPlaybackPosition(word.start * 1000);
    },
    [toggleWord, setPlaybackPosition]
  );

  const handleWordHover = useCallback(
    (word: WordEntry) => {
      setPlaybackPosition(word.start * 1000);
    },
    [setPlaybackPosition]
  );

  const handleTranscribe = async () => {
    if (!sourceVideo) return;
    try {
      await transcribe(sourceVideo);
      addToast({ type: "success", title: "Transcription complete" });
      if (wordSrtPath) {
        await loadTranscript(wordSrtPath);
      }
    } catch (e) {
      addToast({ type: "error", title: "Transcription failed", message: String(e) });
    }
  };

  const handleRemoveFillerWords = () => {
    setIncludedWords((prev) => {
      const next = new Set(prev);
      for (const word of wordEntries) {
        if (word.isFiller) {
          next.delete(word.id);
        }
      }
      return next;
    });
    const count = wordEntries.filter((w) => w.isFiller).length;
    addToast({ type: "success", title: "Filler words removed", message: `Removed ${count} filler words` });
  };

  const handleApplyEdit = async () => {
    if (!sourceVideo) return;

    const included = wordEntries.filter((w) => includedWords.has(w.id));
    if (included.length === 0) {
      addToast({ type: "error", title: "No words selected" });
      return;
    }

    const segments = included.map((w) => ({
      start: w.start,
      end: w.end,
      text: w.text,
    }));

    setIsApplying(true);
    try {
      await applyEdit(sourceVideo, segments);
      addToast({ type: "success", title: "Edit applied", message: `${segments.length} words rendered` });
    } catch (e) {
      addToast({ type: "error", title: "Edit failed", message: String(e) });
    } finally {
      setIsApplying(false);
    }
  };

  if (entries.length === 0) {
    return (
      <div className="flex h-full flex-col">
        <div className="border-b p-3">
          <h3 className="text-sm font-medium">Transcript</h3>
        </div>
        <div className="flex flex-1 flex-col items-center justify-center p-6 gap-4">
          <p className="text-center text-sm text-muted-foreground">
            No transcript yet. Transcribe your video to get started.
          </p>
          <button
            onClick={handleTranscribe}
            disabled={isTranscribing || !sourceVideo}
            className="flex items-center gap-2 rounded-md bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
          >
            <Mic className="h-4 w-4" />
            {isTranscribing ? `Transcribing... ${transcriptionProgress}%` : "Transcribe"}
          </button>
        </div>
      </div>
    );
  }

  const segments = useMemo(() => {
    const groups: Map<number, WordEntry[]> = new Map();
    for (const word of wordEntries) {
      if (!groups.has(word.segmentIndex)) {
        groups.set(word.segmentIndex, []);
      }
      groups.get(word.segmentIndex)!.push(word);
    }
    return groups;
  }, [wordEntries]);

  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center justify-between border-b p-3 shrink-0">
        <h3 className="text-sm font-medium">Transcript — Click words to clip</h3>
        <div className="flex items-center gap-2">
          <span className="text-xs text-muted-foreground">
            {includedWords.size}/{wordEntries.length} words
          </span>
          <button
            onClick={handleRemoveFillerWords}
            className="rounded-md p-1.5 text-muted-foreground hover:bg-secondary hover:text-foreground"
            title="Remove all filler words"
          >
            <Eraser className="h-4 w-4" />
          </button>
          <button
            onClick={handleApplyEdit}
            disabled={isApplying}
            className="flex items-center gap-1.5 rounded-md bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
            title="Apply edits and render"
          >
            <Save className="h-3.5 w-3.5" />
            {isApplying ? "Rendering..." : "Apply & Render"}
          </button>
        </div>
      </div>

      <div className="flex-1 overflow-y-auto p-4 space-y-3">
        {Array.from(segments.entries()).map(([segIdx, words]) => (
          <div key={segIdx} className="leading-relaxed">
            {words.map((word) => {
              const isIncluded = includedWords.has(word.id);
              return (
                <span
                  key={word.id}
                  className={`inline cursor-pointer rounded px-0.5 py-0.5 transition-all ${
                    !isIncluded
                      ? "line-through opacity-30 bg-red-500/20"
                      : word.isFiller
                        ? "bg-yellow-200/30 hover:bg-yellow-200/50"
                        : "hover:bg-secondary"
                  }`}
                  onClick={() => handleWordClick(word)}
                  onMouseEnter={() => handleWordHover(word)}
                  title={`${word.start.toFixed(1)}s - ${word.end.toFixed(1)}s`}
                >
                  {word.text}
                </span>
              );
            })}
          </div>
        ))}
      </div>

      <div className="border-t p-2 text-xs text-muted-foreground shrink-0 flex justify-between">
        <span>{segments.size} segments</span>
        <span>{wordEntries.length - includedWords.size} words will be clipped</span>
      </div>
    </div>
  );
}
