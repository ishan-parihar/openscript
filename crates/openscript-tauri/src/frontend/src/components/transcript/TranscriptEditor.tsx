import { useEditor } from "@tiptap/react";
import StarterKit from "@tiptap/starter-kit";
import { EditorContent } from "@tiptap/react";
import { useTranscriptStore } from "../../store/transcript";
import { useEditorStore } from "../../store/editor";
import { Eraser } from "lucide-react";

export function TranscriptEditor() {
  const { entries, fillerAnalysis, removeFillerWords } = useTranscriptStore();
  const { activePanel } = useEditorStore();

  const editor = useEditor({
    extensions: [StarterKit],
    content: entries.length
      ? entries.map((e) => `<p>${e.text}</p>`).join("")
      : "<p>No transcript yet</p>",
    editable: true,
  });

  if (activePanel !== "transcript") return null;

  if (!entries.length) {
    return (
      <div className="flex h-full flex-col">
        <div className="border-b p-3">
          <h3 className="text-sm font-medium">Transcript</h3>
        </div>
        <div className="flex flex-1 items-center justify-center p-6">
          <p className="text-center text-sm text-muted-foreground">
            No transcript yet. Transcribe a video to get started.
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center justify-between border-b p-3">
        <h3 className="text-sm font-medium">Transcript</h3>
        <div className="flex items-center gap-2">
          {fillerAnalysis && (
            <span className="rounded-full bg-yellow-100 px-2 py-0.5 text-xs text-yellow-800">
              {fillerAnalysis.filler_word_count} filler words
            </span>
          )}
          <button
            onClick={removeFillerWords}
            className="rounded-md p-1.5 text-muted-foreground hover:bg-secondary hover:text-foreground"
            title="Remove all filler words"
          >
            <Eraser className="h-4 w-4" />
          </button>
        </div>
      </div>
      <div className="flex-1 overflow-y-auto p-3">
        <EditorContent editor={editor} />
      </div>
    </div>
  );
}
