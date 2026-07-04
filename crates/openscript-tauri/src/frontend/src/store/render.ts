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
    set({
      isRendering: true,
      progress: 0,
      status: "Starting render...",
      error: null,
      outputPath: null,
      fileSizeBytes: null,
      durationMs: null,
      etaSeconds: null,
    });

    // Clear any leftover poller.
    if (pollInterval) {
      clearInterval(pollInterval);
      pollInterval = null;
    }

    // Start polling BEFORE firing the render. Prior versions awaited
    // renderTimeline first, which meant the poller only ever saw the
    // terminal state — the progress bar was useless. Now the poller runs
    // concurrently with the render and can observe intermediate progress
    // and a user-initiated cancellation.
    pollInterval = setInterval(async () => {
      try {
        const p = await api.getRenderProgress();
        set({
          progress: p.progress,
          status: p.status,
        });

        if (p.status === "completed" || p.status === "cancelled" || p.status === "idle") {
          if (pollInterval) {
            clearInterval(pollInterval);
            pollInterval = null;
          }
          set({ isRendering: false });
        }
      } catch {
        if (pollInterval) {
          clearInterval(pollInterval);
          pollInterval = null;
        }
        set({ isRendering: false, error: "Failed to poll render progress" });
      }
    }, 500);

    // Fire the render without synchronously awaiting it. The poller above
    // will observe completion. The .then/.catch handlers update the final
    // output_path / file_size / error state.
    api
      .renderTimeline({ quality })
      .then((result) => {
        if (result.status === "cancelled") {
          set({
            isRendering: false,
            progress: 0,
            status: "Cancelled",
            outputPath: null,
            fileSizeBytes: null,
            durationMs: null,
          });
        } else {
          set({
            outputPath: result.output_path,
            fileSizeBytes: result.file_size_bytes,
            durationMs: result.duration_ms ?? null,
            progress: 100,
            status: "Completed",
            isRendering: false,
          });
        }
        if (pollInterval) {
          clearInterval(pollInterval);
          pollInterval = null;
        }
      })
      .catch((e: unknown) => {
        set({
          isRendering: false,
          error: e instanceof Error ? e.message : "Render failed",
        });
        if (pollInterval) {
          clearInterval(pollInterval);
          pollInterval = null;
        }
      });
  },

  cancelRender: async () => {
    try {
      await api.cancelRender();
    } catch {
      // ignore — the poller will still observe the cancelled status
    }
    // Don't clear the poller here; let it observe the cancelled status and
    // clean up. This avoids a race where the UI shows "cancelled" before
    // ffmpeg has actually been killed.
    set({ status: "Cancelling..." });
  },
}));
