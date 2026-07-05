import { create } from "zustand";
import * as api from "../lib/tauri";
import { useProjectStore } from "./project";

export interface ChatMessage {
  id: string;
  role: "user" | "assistant" | "system";
  content: string;
  /** Optional tool suggestions attached to an assistant message. */
  suggestions?: ToolSuggestion[];
  /** Optional tool execution result attached to a system message. */
  toolResult?: { name: string; result: unknown; error?: string };
  timestamp: number;
}

export interface ToolSuggestion {
  name: string;
  relevance: number;
  description: string;
}

interface AIState {
  messages: ChatMessage[];
  isProcessing: boolean;
  /** When the user picks a suggestion, the store loads its inputSchema and
   *  stores it here so the UI can render an args form. */
  pendingTool: { name: string; description: string; schema: Record<string, unknown> } | null;

  /** User typed a natural-language request. Calls help.tool to find tools. */
  sendMessage: (content: string) => Promise<void>;
  /** User picked a suggested tool. Loads its schema for the args form. */
  selectTool: (suggestion: ToolSuggestion) => Promise<void>;
  /** User filled the args form and clicked Execute. Calls invokeTool. */
  executeTool: (args: Record<string, unknown>) => Promise<void>;
  /** Cancel the pending tool (dismiss the args form). */
  cancelTool: () => void;
  /** Quick action: run the golden trajectory (script.to_video). */
  runGoldenTrajectory: (scriptInput: string) => Promise<void>;
  /** Quick action: probe system capabilities. */
  probeCapabilities: () => Promise<void>;
  clear: () => void;
}

const QUICK_STARTS = [
  "Probe what subsystems are available",
  "Create a video from a script",
  "Transcribe this video",
  "Add background music",
];

export const useAIStore = create<AIState>((set, get) => ({
  messages: [],
  isProcessing: false,
  pendingTool: null,

  sendMessage: async (content: string) => {
    const userMsg: ChatMessage = {
      id: crypto.randomUUID(),
      role: "user",
      content,
      timestamp: Date.now(),
    };
    set({ messages: [...get().messages, userMsg], isProcessing: true });

    try {
      // Route natural-language queries for the "quick start" buttons directly
      // to the right tool, skipping help.tool.
      const lower = content.toLowerCase();
      if (lower.includes("probe") && lower.includes("subsystem")) {
        await get().probeCapabilities();
        return;
      }
      if (lower.includes("create") && lower.includes("video from a script")) {
        // Defer to the runGoldenTrajectory flow — the user will be prompted
        // for a script via the args form.
        const schema = await api.getMcpTool("script.to_video");
        if (schema) {
          set({
            isProcessing: false,
            pendingTool: { name: schema.name, description: schema.description, schema: schema.inputSchema },
          });
          const assistantMsg: ChatMessage = {
            id: crypto.randomUUID(),
            role: "assistant",
            content: "I'll run script.to_video for you. Fill in the script (and optional output path / caption style) below.",
            timestamp: Date.now(),
          };
          set({ messages: [...get().messages, assistantMsg] });
        }
        return;
      }

      // Default: call help.tool to find relevant MCP tools.
      const result = await api.helpTool(content, 6);
      const assistantMsg: ChatMessage = {
        id: crypto.randomUUID(),
        role: "assistant",
        content:
          result.count > 0
            ? `Here are the most relevant tools for "${content}". Click one to execute it:`
            : `No tools matched "${content}". Try rephrasing — e.g. "add voiceover", "burn captions", "download background music".`,
        suggestions: result.results,
        timestamp: Date.now(),
      };
      set({ messages: [...get().messages, assistantMsg], isProcessing: false });
    } catch (e) {
      const errorMsg: ChatMessage = {
        id: crypto.randomUUID(),
        role: "system",
        content: `Error finding tools: ${String(e)}`,
        timestamp: Date.now(),
      };
      set({ messages: [...get().messages, errorMsg], isProcessing: false });
    }
  },

  selectTool: async (suggestion: ToolSuggestion) => {
    set({ isProcessing: true });
    try {
      const schema = await api.getMcpTool(suggestion.name);
      if (!schema) {
        const msg: ChatMessage = {
          id: crypto.randomUUID(),
          role: "system",
          content: `Tool "${suggestion.name}" not found in the registry.`,
          timestamp: Date.now(),
        };
        set({ messages: [...get().messages, msg], isProcessing: false });
        return;
      }
      set({
        pendingTool: {
          name: schema.name,
          description: schema.description,
          schema: schema.inputSchema,
        },
        isProcessing: false,
      });
    } catch (e) {
      const msg: ChatMessage = {
        id: crypto.randomUUID(),
        role: "system",
        content: `Error loading tool schema: ${String(e)}`,
        timestamp: Date.now(),
      };
      set({ messages: [...get().messages, msg], isProcessing: false });
    }
  },

  executeTool: async (args: Record<string, unknown>) => {
    const pending = get().pendingTool;
    if (!pending) return;
    set({ isProcessing: true, pendingTool: null });

    try {
      const result = await api.invokeTool<unknown>(pending.name, args);
      const resultMsg: ChatMessage = {
        id: crypto.randomUUID(),
        role: "system",
        content: `Tool ${pending.name} completed.`,
        toolResult: { name: pending.name, result },
        timestamp: Date.now(),
      };
      set({ messages: [...get().messages, resultMsg], isProcessing: false });
      // If this was a timeline-mutating tool, refresh the project timeline.
      if (pending.name.startsWith("timeline.") || pending.name.startsWith("script.to_") || pending.name === "broll.director") {
        await useProjectStore.getState().refreshTimeline();
      }
    } catch (e) {
      const errorMsg: ChatMessage = {
        id: crypto.randomUUID(),
        role: "system",
        content: `Tool ${pending.name} failed: ${String(e)}`,
        toolResult: { name: pending.name, result: null, error: String(e) },
        timestamp: Date.now(),
      };
      set({ messages: [...get().messages, errorMsg], isProcessing: false });
    }
  },

  cancelTool: () => set({ pendingTool: null }),

  runGoldenTrajectory: async (scriptInput: string) => {
    set({ isProcessing: true });
    try {
      const result = await api.scriptToVideo(scriptInput);
      const msg: ChatMessage = {
        id: crypto.randomUUID(),
        role: "system",
        content: `Video created! ${result.scenes_rendered} scenes, ${result.duration_s.toFixed(1)}s. Output: ${result.output_path}`,
        toolResult: { name: "script.to_video", result },
        timestamp: Date.now(),
      };
      set({ messages: [...get().messages, msg], isProcessing: false });
    } catch (e) {
      const msg: ChatMessage = {
        id: crypto.randomUUID(),
        role: "system",
        content: `script.to_video failed: ${String(e)}`,
        toolResult: { name: "script.to_video", result: null, error: String(e) },
        timestamp: Date.now(),
      };
      set({ messages: [...get().messages, msg], isProcessing: false });
    }
  },

  probeCapabilities: async () => {
    set({ isProcessing: true });
    try {
      const caps = await api.systemCapabilities();
      const lines: string[] = ["Subsystem availability:"];
      const subs: Array<[string, { available: boolean; reason?: string | null }]> = [
        ["ffmpeg", caps.ffmpeg],
        ["kokoro", caps.kokoro],
        ["transcription", caps.transcription],
        ["voicebox", caps.voicebox],
        ["pexels", caps.pexels],
        ["giphy", caps.giphy],
        ["pixabay", caps.pixabay],
        ["sfx_library", caps.sfx_library],
        ["music_library", caps.music_library],
        ["hyperframes", caps.hyperframes],
      ];
      for (const [name, info] of subs) {
        const mark = info.available ? "✓" : "✗";
        const detail = info.available ? "" : ` (${info.reason ?? "not configured"})`;
        lines.push(`  ${mark} ${name}${detail}`);
      }
      const msg: ChatMessage = {
        id: crypto.randomUUID(),
        role: "system",
        content: lines.join("\n"),
        toolResult: { name: "system.capabilities", result: caps },
        timestamp: Date.now(),
      };
      set({ messages: [...get().messages, msg], isProcessing: false });
    } catch (e) {
      const msg: ChatMessage = {
        id: crypto.randomUUID(),
        role: "system",
        content: `system.capabilities failed: ${String(e)}`,
        toolResult: { name: "system.capabilities", result: null, error: String(e) },
        timestamp: Date.now(),
      };
      set({ messages: [...get().messages, msg], isProcessing: false });
    }
  },

  clear: () => set({ messages: [], pendingTool: null, isProcessing: false }),
}));

export { QUICK_STARTS };
