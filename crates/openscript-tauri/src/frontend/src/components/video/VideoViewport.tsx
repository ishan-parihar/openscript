import { useRef, useEffect, useCallback, useState } from "react";
import { useEditorStore } from "../../store/editor";
import { useProjectStore } from "../../store/project";

function mediaSrc(path: string): string {
  const encoded = path.split("/").map(encodeURIComponent).join("/");
  return `http://127.0.0.1:1421/file/${encoded}`;
}

export function VideoViewport() {
  const videoRef = useRef<HTMLVideoElement>(null);
  const { sourceVideo } = useProjectStore();
  const { isPlaying, playbackPosition, setIsPlaying, setPlaybackPosition } = useEditorStore();
  const [videoError, setVideoError] = useState<string | null>(null);

  useEffect(() => {
    if (sourceVideo) {
      const url = mediaSrc(sourceVideo);
      console.log('[VideoViewport] sourceVideo:', sourceVideo);
      console.log('[VideoViewport] file:// URL:', url);
    }
  }, [sourceVideo]);

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

  const handleError = useCallback(() => {
    const video = videoRef.current;
    if (!video) return;
    const err = video.error;
    if (err) {
      switch (err.code) {
        case MediaError.MEDIA_ERR_SRC_NOT_SUPPORTED:
          setVideoError("Video format not supported. Check that GStreamer plugins are installed.");
          break;
        case MediaError.MEDIA_ERR_NETWORK:
          setVideoError("Failed to load video. Check that the file path is accessible.");
          break;
        case MediaError.MEDIA_ERR_DECODE:
          setVideoError("Video decode failed. Missing GStreamer codecs? Install gstreamer1.0-plugins-good.");
          break;
        default:
          setVideoError("Failed to load video.");
      }
    } else {
      setVideoError("Failed to load video.");
    }
  }, []);

  if (!sourceVideo) {
    return (
      <div className="flex h-full items-center justify-center bg-black/50">
        <p className="text-sm text-muted-foreground">Open a video to begin</p>
      </div>
    );
  }

  if (videoError) {
    return (
      <div className="flex h-full flex-col items-center justify-center bg-black/50 p-6">
        <p className="text-sm text-red-400 mb-2 text-center">{videoError}</p>
        <p className="text-xs text-muted-foreground text-center max-w-md">
          On Linux, install: <code className="bg-white/10 px-1 rounded">sudo apt install gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-libav</code>
        </p>
        <button
          onClick={() => setVideoError(null)}
          className="mt-4 text-xs text-muted-foreground hover:text-foreground underline"
        >
          Dismiss
        </button>
      </div>
    );
  }

  return (
    <div className="flex h-full flex-col bg-black">
      <div className="relative flex-1 flex items-center justify-center overflow-hidden">
        <video
          ref={videoRef}
          src={mediaSrc(sourceVideo)}
          className="max-h-full max-w-full object-contain"
          onTimeUpdate={handleTimeUpdate}
          onEnded={handleEnded}
          onError={handleError}
          onClick={() => setIsPlaying(!isPlaying)}
        />
      </div>
    </div>
  );
}
