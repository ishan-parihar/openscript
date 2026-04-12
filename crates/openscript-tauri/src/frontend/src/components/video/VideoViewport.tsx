import { useRef, useEffect, useCallback } from "react";
import { useEditorStore } from "../../store/editor";
import { useProjectStore } from "../../store/project";
import { convertFileSrc } from "@tauri-apps/api/core";

export function VideoViewport() {
  const videoRef = useRef<HTMLVideoElement>(null);
  const { sourceVideo } = useProjectStore();
  const { isPlaying, playbackPosition, setIsPlaying, setPlaybackPosition } = useEditorStore();

  useEffect(() => {
    const video = videoRef.current;
    if (!video) return;

    if (isPlaying) {
      video.play().catch(() => setIsPlaying(false));
    } else {
      video.pause();
    }
  }, [isPlaying, setIsPlaying]);

  useEffect(() => {
    const video = videoRef.current;
    if (!video) return;
    if (Math.abs(video.currentTime * 1000 - playbackPosition) > 200) {
      video.currentTime = playbackPosition / 1000;
    }
  }, [playbackPosition]);

  const handleTimeUpdate = useCallback(() => {
    const video = videoRef.current;
    if (!video) return;
    setPlaybackPosition(video.currentTime * 1000);
  }, [setPlaybackPosition]);

  const handleEnded = useCallback(() => {
    setIsPlaying(false);
    setPlaybackPosition(0);
  }, [setIsPlaying, setPlaybackPosition]);

  if (!sourceVideo) {
    return (
      <div className="flex h-full items-center justify-center bg-black/50">
        <p className="text-sm text-muted-foreground">Open a video to begin</p>
      </div>
    );
  }

  return (
    <div className="flex h-full flex-col bg-black">
      <div className="relative flex-1 flex items-center justify-center overflow-hidden">
        <video
          ref={videoRef}
          src={convertFileSrc(sourceVideo)}
          className="max-h-full max-w-full object-contain"
          onTimeUpdate={handleTimeUpdate}
          onEnded={handleEnded}
          onClick={() => setIsPlaying(!isPlaying)}
        />
      </div>
    </div>
  );
}
