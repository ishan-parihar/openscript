import { create } from "zustand";
import * as api from "../lib/tauri";

type RenderQuality = "preview" | "standard" | "high";

export interface RenderState {
  quality: RenderQuality;
  includeCaptions: boolean;
  includeBroll: boolean;
  includeMusic: boolean;
  includeSFX: boolean;
  isRendering: boolean;
  progress: number;
  status: string;
  etaSeconds: number | null;
  outputPath: string | null;
  fileSizeBytes: number | null;
  durationMs: number | null;
  error: string | null;

  setQuality: (q: RenderQuality) => void;
  toggleCaptions: () => void;
  toggleBroll: () => void;
  toggleMusic: () => void;
  toggleSFX: () => void;
  render: () => Promise<void>;
  cancelRender: () => Promise<void>;
}

let pollInterval: ReturnType<typeof setInterval> | null = null;

export const useRenderStore = create<RenderState>((set, get) => ({
  quality: "standard",
  includeCaptions: true,
  includeBroll: true,
  includeMusic: true,
  includeSFX: true,
  isRendering: false,
  progress: 0,
  status: "",
  etaSeconds: null,
  outputPath: null,
  fileSizeBytes: null,
  durationMs: null,
  error: null,

  setQuality: (q) => set({ quality: q }),
  toggleCaptions: () => set((s) => ({ includeCaptions: !s.includeCaptions })),
  toggleBroll: () => set((s) => ({ includeBroll: !s.includeBroll })),
  toggleMusic: () => set((s) => ({ includeMusic: !s.includeMusic })),
  toggleSFX: () => set((s) => ({ includeSFX: !s.includeSFX })),

  render: async () => {
    const { quality } = get();
    set({ isRendering: true, progress: 0, status: "Starting render...", error: null, outputPath: null, fileSizeBytes: null, durationMs: null, etaSeconds: null });

    try {
      const result = await api.renderTimeline({ quality });

      if (pollInterval) clearInterval(pollInterval);

      pollInterval = setInterval(async () => {
        try {
          const progressData = await api.getRenderProgress();
          set({
            progress: progressData.progress,
            status: progressData.status,
            etaSeconds: progressData.eta_seconds ?? null,
          });

          if (progressData.status === "completed") {
            if (pollInterval) clearInterval(pollInterval);
            pollInterval = null;
            set({
              isRendering: false,
              progress: 100,
              status: "Completed",
              outputPath: result.output_path,
              fileSizeBytes: result.file_size_bytes,
              durationMs: result.duration_ms,
            });
          } else if (progressData.status === "error") {
            if (pollInterval) clearInterval(pollInterval);
            pollInterval = null;
            set({ isRendering: false, error: progressData.status });
          }
        } catch {
          if (pollInterval) clearInterval(pollInterval);
          pollInterval = null;
          set({ isRendering: false, error: "Failed to poll render progress" });
        }
      }, 500);
    } catch (e: unknown) {
      set({
        isRendering: false,
        error: e instanceof Error ? e.message : "Render failed",
      });
    }
  },

  cancelRender: async () => {
    await api.cancelRender().catch(() => {});
    if (pollInterval) clearInterval(pollInterval);
    pollInterval = null;
    set({ isRendering: false, status: "Cancelled" });
  },
}));
