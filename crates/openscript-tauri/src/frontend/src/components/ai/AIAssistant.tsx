import { useState } from "react";
import { Sparkles, Send } from "lucide-react";
import { useAIStore } from "../../store/ai";

export function AIAssistant() {
  const { messages, isProcessing, suggestions, sendMessage, clear } = useAIStore();
  const [input, setInput] = useState("");

  const handleSend = () => {
    const trimmed = input.trim();
    if (!trimmed || isProcessing) return;
    sendMessage(trimmed);
    setInput("");
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center justify-between border-b px-4 py-3">
        <div className="flex items-center gap-2">
          <Sparkles className="h-4 w-4 text-primary" />
          <h2 className="text-sm font-semibold">AI Assistant</h2>
        </div>
        {messages.length > 0 && (
          <button
            onClick={clear}
            className="text-xs text-muted-foreground hover:text-foreground"
          >
            Clear
          </button>
        )}
      </div>

      <div className="flex-1 overflow-y-auto px-4 py-4">
        {messages.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-full gap-4">
            <p className="text-sm text-muted-foreground text-center">
              What would you like to do with this video?
            </p>
            <div className="grid grid-cols-1 gap-2 w-full max-w-sm">
              {suggestions.map((s) => (
                <button
                  key={s}
                  onClick={() => sendMessage(s)}
                  className="rounded-lg border px-4 py-3 text-left text-sm hover:border-primary hover:bg-primary/5 transition-colors"
                >
                  {s}
                </button>
              ))}
            </div>
          </div>
        ) : (
          <div className="flex flex-col gap-3">
            {messages.map((msg) => (
              <div
                key={msg.id}
                className={`flex ${msg.role === "user" ? "justify-end" : "justify-start"}`}
              >
                <div
                  className={`max-w-[80%] rounded-xl px-4 py-2.5 text-sm ${
                    msg.role === "user"
                      ? "bg-primary text-primary-foreground"
                      : "bg-secondary text-secondary-foreground"
                  }`}
                >
                  {msg.content}
                </div>
              </div>
            ))}
            {isProcessing && (
              <div className="flex justify-start">
                <div className="rounded-xl bg-secondary px-4 py-2.5 text-sm text-secondary-foreground">
                  <span className="animate-pulse">Thinking...</span>
                </div>
              </div>
            )}
          </div>
        )}
      </div>

      <div className="border-t p-3">
        <div className="flex items-center gap-2">
          <input
            type="text"
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder="Ask AI to edit your video..."
            disabled={isProcessing}
            className="flex-1 rounded-lg border bg-background px-3 py-2 text-sm placeholder:text-muted-foreground focus:outline-none focus:ring-2 focus:ring-primary disabled:opacity-50"
          />
          <button
            onClick={handleSend}
            disabled={isProcessing || !input.trim()}
            className="rounded-lg bg-primary p-2 text-primary-foreground hover:bg-primary/90 disabled:opacity-50 disabled:cursor-not-allowed"
          >
            <Send className="h-4 w-4" />
          </button>
        </div>
      </div>
    </div>
  );
}
