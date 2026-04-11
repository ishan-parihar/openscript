import { useEffect, useRef, useCallback } from "react";
import { useEditorStore } from "../store/editor";
import { useProjectStore } from "../store/project";

interface UsePlaybackOptions {
  fps?: number;
}

export function usePlayback({ fps: _fps = 30 }: UsePlaybackOptions = {}) {
  const { isPlaying, playbackPosition, setPlaybackPosition, setIsPlaying } =
    useEditorStore();
  const { segments } = useProjectStore();
  const animationRef = useRef<number>();
  const lastTimeRef = useRef<number>(0);

  const totalDuration = segments.reduce(
    (max, s) => Math.max(max, s.source_end_ms),
    0
  );

  const animate = useCallback(
    (time: number) => {
      if (!lastTimeRef.current) lastTimeRef.current = time;
      const delta = time - lastTimeRef.current;
      lastTimeRef.current = time;

      setPlaybackPosition(playbackPosition + delta);

      if (playbackPosition >= totalDuration) {
        setIsPlaying(false);
        setPlaybackPosition(0);
        return;
      }

      animationRef.current = requestAnimationFrame(animate);
    },
    [playbackPosition, totalDuration, setPlaybackPosition, setIsPlaying]
  );

  useEffect(() => {
    if (isPlaying) {
      lastTimeRef.current = 0;
      animationRef.current = requestAnimationFrame(animate);
    } else {
      if (animationRef.current) {
        cancelAnimationFrame(animationRef.current);
      }
    }

    return () => {
      if (animationRef.current) {
        cancelAnimationFrame(animationRef.current);
      }
    };
  }, [isPlaying, animate]);

  const seek = useCallback(
    (ms: number) => {
      setPlaybackPosition(Math.max(0, Math.min(ms, totalDuration)));
    },
    [totalDuration, setPlaybackPosition]
  );

  const togglePlayback = useCallback(() => {
    setIsPlaying(!isPlaying);
  }, [isPlaying, setIsPlaying]);

  return { seek, togglePlayback, totalDuration };
}
