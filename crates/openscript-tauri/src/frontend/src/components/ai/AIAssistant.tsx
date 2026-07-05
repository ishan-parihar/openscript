import { useState } from "react";
import { Sparkles, Send, X, Play, Wrench } from "lucide-react";
import { useAIStore, QUICK_STARTS } from "../../store/ai";

/** Render a tool's inputSchema as a simple form. Supports the common cases:
 *  string, number, boolean, and optional fields. Nested objects fall back to
 *  a JSON textarea. */
function ArgsForm({
  schema,
  onExecute,
  onCancel,
  isProcessing,
}: {
  schema: Record<string, unknown>;
  onExecute: (args: Record<string, unknown>) => void;
  onCancel: () => void;
  isProcessing: boolean;
}) {
  const props = (schema.properties ?? {}) as Record<string, { type: string; description?: string; default?: unknown; anyOf?: Array<{ type: string }> }>;
  const required = new Set((schema.required ?? []) as string[]);
  const [values, setValues] = useState<Record<string, unknown>>({});

  const setField = (key: string, val: unknown) => setValues((v) => ({ ...v, [key]: val }));

  const handleSubmit = () => {
    // Drop undefined values so they're not sent as null to Rust.
    const clean: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(values)) {
      if (v !== undefined && v !== "") clean[k] = v;
    }
    onExecute(clean);
  };

  return (
    <div className="rounded-xl border bg-card p-4 space-y-3">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2 text-sm font-medium">
          <Wrench className="h-3.5 w-3.5 text-primary" />
          Fill in the arguments
        </div>
        <button onClick={onCancel} className="text-muted-foreground hover:text-foreground">
          <X className="h-3.5 w-3.5" />
        </button>
      </div>
      {Object.entries(props).map(([key, prop]) => {
        // Handle anyOf (Option<T> in Rust → nullable in JSON schema)
        const isOptional = !required.has(key);
        const typeStr = prop.type ?? prop.anyOf?.[0]?.type ?? "string";
        const desc = prop.description ?? "";
        return (
          <div key={key} className="space-y-1">
            <label className="text-xs font-medium text-muted-foreground">
              {key}
              {isOptional ? <span className="ml-1 text-muted-foreground/60">(optional)</span> : <span className="ml-1 text-destructive">*</span>}
            </label>
            {typeStr === "boolean" ? (
              <select
                value={values[key] === undefined ? "" : String(values[key])}
                onChange={(e) => setField(key, e.target.value === "" ? undefined : e.target.value === "true")}
                className="w-full rounded-md border bg-background px-2 py-1.5 text-xs"
              >
                <option value="">— unset —</option>
                <option value="true">true</option>
                <option value="false">false</option>
              </select>
            ) : typeStr === "integer" || typeStr === "number" ? (
              <input
                type="number"
                value={(values[key] as number | undefined) ?? ""}
                onChange={(e) => setField(key, e.target.value === "" ? undefined : Number(e.target.value))}
                placeholder={desc}
                className="w-full rounded-md border bg-background px-2 py-1.5 text-xs"
              />
            ) : (
              <input
                type="text"
                value={(values[key] as string | undefined) ?? ""}
                onChange={(e) => setField(key, e.target.value || undefined)}
                placeholder={desc || `Enter ${key}`}
                className="w-full rounded-md border bg-background px-2 py-1.5 text-xs"
              />
            )}
          </div>
        );
      })}
      <div className="flex justify-end gap-2 pt-1">
        <button onClick={onCancel} className="rounded-md px-3 py-1.5 text-xs text-muted-foreground hover:bg-secondary">
          Cancel
        </button>
        <button
          onClick={handleSubmit}
          disabled={isProcessing}
          className="rounded-md bg-primary px-3 py-1.5 text-xs text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
        >
          {isProcessing ? "Executing..." : "Execute"}
        </button>
      </div>
    </div>
  );
}

/** Pretty-print a tool result. If it's a string, show it directly; if it's an
 *  object, show the JSON collapsed by default with a toggle. */
function ResultView({ result, error }: { result: unknown; error?: string }) {
  const [expanded, setExpanded] = useState(false);
  if (error) {
    return <pre className="mt-1 whitespace-pre-wrap text-xs text-destructive">{error}</pre>;
  }
  if (typeof result === "string") {
    return <pre className="mt-1 whitespace-pre-wrap text-xs">{result}</pre>;
  }
  const json = JSON.stringify(result, null, 2);
  if (json.length < 200) {
    return <pre className="mt-1 whitespace-pre-wrap text-xs text-muted-foreground">{json}</pre>;
  }
  return (
    <div className="mt-1">
      <pre className="whitespace-pre-wrap text-xs text-muted-foreground">
        {expanded ? json : json.slice(0, 200) + "..."}
      </pre>
      <button
        onClick={() => setExpanded((e) => !e)}
        className="text-xs text-primary hover:underline"
      >
        {expanded ? "Show less" : "Show more"}
      </button>
    </div>
  );
}

export function AIAssistant() {
  const { messages, isProcessing, pendingTool, sendMessage, selectTool, executeTool, cancelTool, clear } = useAIStore();
  const [input, setInput] = useState("");

  const handleSend = async () => {
    const trimmed = input.trim();
    if (!trimmed || isProcessing) return;
    await sendMessage(trimmed);
    setInput("");
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      void handleSend();
    }
  };

  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center justify-between border-b px-4 py-3">
        <div className="flex items-center gap-2">
          <Sparkles className="h-4 w-4 text-primary" />
          <h2 className="text-sm font-semibold">Command Palette</h2>
        </div>
        {messages.length > 0 && (
          <button onClick={clear} className="text-xs text-muted-foreground hover:text-foreground">
            Clear
          </button>
        )}
      </div>

      <div className="flex-1 overflow-y-auto px-4 py-4">
        {messages.length === 0 && !pendingTool ? (
          <div className="flex flex-col items-center justify-center h-full gap-4">
            <p className="text-sm text-muted-foreground text-center">
              Describe what you want to do, or pick a quick start.
            </p>
            <div className="grid grid-cols-1 gap-2 w-full max-w-sm">
              {QUICK_STARTS.map((s) => (
                <button
                  key={s}
                  onClick={() => void sendMessage(s)}
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
              <div key={msg.id} className={`flex ${msg.role === "user" ? "justify-end" : "justify-start"}`}>
                <div
                  className={`max-w-[85%] rounded-xl px-4 py-2.5 text-sm ${
                    msg.role === "user"
                      ? "bg-primary text-primary-foreground"
                      : msg.role === "assistant"
                      ? "bg-secondary text-secondary-foreground"
                      : "bg-muted text-muted-foreground border"
                  }`}
                >
                  <pre className="whitespace-pre-wrap font-sans">{msg.content}</pre>
                  {msg.suggestions && msg.suggestions.length > 0 && (
                    <div className="mt-2 space-y-1">
                      {msg.suggestions.map((s) => (
                        <button
                          key={s.name}
                          onClick={() => void selectTool(s)}
                          className="block w-full rounded-md border bg-background px-3 py-2 text-left text-xs hover:border-primary hover:bg-primary/5"
                        >
                          <div className="flex items-center justify-between">
                            <span className="font-mono font-medium">{s.name}</span>
                            <span className="text-muted-foreground">{Math.round(s.relevance * 100)}%</span>
                          </div>
                          <div className="text-muted-foreground mt-0.5">{s.description}</div>
                        </button>
                      ))}
                    </div>
                  )}
                  {msg.toolResult && <ResultView result={msg.toolResult.result} error={msg.toolResult.error} />}
                </div>
              </div>
            ))}
            {pendingTool && (
              <ArgsForm
                schema={pendingTool.schema}
                onExecute={(args) => void executeTool(args)}
                onCancel={cancelTool}
                isProcessing={isProcessing}
              />
            )}
            {isProcessing && !pendingTool && (
              <div className="flex justify-start">
                <div className="rounded-xl bg-secondary px-4 py-2.5 text-sm text-secondary-foreground">
                  <span className="animate-pulse">Working...</span>
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
            placeholder="Describe what you want to do..."
            disabled={isProcessing}
            className="flex-1 rounded-lg border bg-background px-3 py-2 text-sm placeholder:text-muted-foreground focus:outline-none focus:ring-2 focus:ring-primary disabled:opacity-50"
          />
          <button
            onClick={() => void handleSend()}
            disabled={isProcessing || !input.trim()}
            className="rounded-lg bg-primary p-2 text-primary-foreground hover:bg-primary/90 disabled:opacity-50 disabled:cursor-not-allowed"
            title={pendingTool ? "Execute tool" : "Send message"}
          >
            {pendingTool ? <Play className="h-4 w-4" /> : <Send className="h-4 w-4" />}
          </button>
        </div>
      </div>
    </div>
  );
}
