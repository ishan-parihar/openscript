/**
 * PR #214 Runtime Interop — Bundle Entry Point Template
 *
 * This file is the entry point for the esbuild bundle that mounts a Remotion
 * <Player> inside a HyperFrames composition. It is a TEMPLATE — copy it,
 * import your actual composition component, and adjust the props.
 *
 * Build:
 *   npx esbuild entry.tsx --bundle --outfile=dist/bundle.js --format=iife --jsx=automatic
 *
 * The produced bundle.js is referenced by interop/index.html and loaded
 * by HyperFrames' headless browser during rendering.
 */

import React, { useEffect, useRef } from "react";
import { createRoot } from "react-dom/client";
import { Player, type PlayerRef } from "@remotion/player";

// Import your actual Remotion composition here
// import { MyComposition } from "../remotion/src/compositions/MyComposition";

// Placeholder — replace with your actual composition
const PlaceholderComposition: React.FC = () => {
  return React.createElement("div", {
    style: {
      width: "100%",
      height: "100%",
      background: "#1a1a2e",
      color: "#fff",
      display: "flex",
      alignItems: "center",
      justifyContent: "center",
      fontFamily: "system-ui, sans-serif",
      fontSize: "48px",
    },
  }, "Remotion Interop Placeholder");
};

// Composition config — adjust to match your Remotion <Composition> props
const COMPOSITION_ID = "remotion-interop";
const DURATION_IN_FRAMES = 900; // 30s @ 30fps
const FPS = 30;
const WIDTH = 1080;
const HEIGHT = 1920;

const InteropEntry: React.FC = () => {
  const playerRef = useRef<PlayerRef>(null);
  const hostRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!hostRef.current) return;

    const root = createRoot(hostRef.current);

    root.render(
      React.createElement(Player, {
        ref: playerRef,
        component: PlaceholderComposition,
        durationInFrames: DURATION_IN_FRAMES,
        fps: FPS,
        compositionWidth: WIDTH,
        compositionHeight: HEIGHT,
        style: {
          width: "100%",
          height: "100%",
        },
        // Start paused — HF drives the frame via seekTo
        initialFrame: 0,
        autoPlay: false,
        loop: false,
        controls: false,
        clickToPlay: false,
        doubleClickToFullscreen: false,
        spaceKeyToPlayOrPause: false,
        numberOfSharedAudioTags: 0,
        acknowledgeRemotionLicense: true,
      })
    );

    // Register the player on window.__hfRemotion per PR #214
    // HF's runtime calls __hfRemotionSeek(frame) on each tick
    const player = playerRef.current;

    if (player) {
      // Pause immediately — HF drives seeking
      player.pause();

      // Register the seek function
      window.__hfRemotion = window.__hfRemotion || [];
      window.__hfRemotion.push({
        seekTo: (frame: number) => {
          try {
            player.seekTo(frame);
          } catch (e) {
            console.error("[hf-interop] seekTo failed:", e);
          }
        },
        pause: () => player.pause(),
        durationInFrames: DURATION_IN_FRAMES,
        fps: FPS,
      });

      // Also set the global seek function (called by interop/index.html)
      window.__hfRemotionSeek = (frame: number) => {
        try {
          player.seekTo(frame);
        } catch (e) {
          console.error("[hf-interop] __hfRemotionSeek failed:", e);
        }
      };
    }

    return () => {
      root.unmount();
    };
  }, []);

  return React.createElement("div", {
    ref: hostRef,
    id: "remotion-player-host",
    style: { width: "100%", height: "100%" },
  });
};

// Mount the entry point
const hostElement = document.getElementById("remotion-player-host");
if (hostElement) {
  const root = createRoot(hostElement);
  root.render(React.createElement(InteropEntry));
}

// Type declarations for the global interop interface
declare global {
  interface Window {
    __hfRemotion?: Array<{
      seekTo: (frame: number) => void;
      pause: () => void;
      durationInFrames: number;
      fps: number;
    }>;
    __hfRemotionSeek?: (frame: number) => void;
  }
}
