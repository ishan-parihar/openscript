import { Play, Pause, SkipBack, SkipForward, Volume2, VolumeX } from "lucide-react";
import { useEditorStore } from "../../store/editor";
import { useProjectStore } from "../../store/project";
import { useCallback, useState, useEffect, useRef } from "react";

export function PlaybackControls() {
  const { isPlaying, setIsPlaying, playbackPosition, setPlaybackPosition } = useEditorStore();
  const { sourceVideo } = useProjectStore();
  const [isMuted, setIsMuted] = useState(false);
  const [playbackRate, setPlaybackRate] = useState(1);
  const [durationMs, setDurationMs] = useState(60000);
  const videoRef = useRef<HTMLVideoElement | null>(null);

  useEffect(() => {
    const video = document.querySelector("video") as HTMLVideoElement | null;
    if (video) {
      videoRef.current = video;
      video.muted = isMuted;
      video.playbackRate = playbackRate;

      const onLoadedMetadata = () => setDurationMs(video.duration * 1000);
      video.addEventListener("loadedmetadata", onLoadedMetadata);
      return () => video.removeEventListener("loadedmetadata", onLoadedMetadata);
    }
  }, [isMuted, playbackRate, sourceVideo]);

  const formatTime = (ms: number) => {
    const totalSeconds = Math.floor(ms / 1000);
    const mins = Math.floor(totalSeconds / 60);
    const secs = totalSeconds % 60;
    return `${mins}:${secs.toString().padStart(2, "0")}`;
  };

  const handleSeek = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => {
      const ms = Number(e.target.value);
      setPlaybackPosition(ms);
      if (videoRef.current) {
        videoRef.current.currentTime = ms / 1000;
      }
    },
    [setPlaybackPosition]
  );

  const handleSkipBack = useCallback(() => {
    setPlaybackPosition(Math.max(0, playbackPosition - 5000));
  }, [playbackPosition, setPlaybackPosition]);

  const handleSkipForward = useCallback(() => {
    setPlaybackPosition(Math.min(durationMs, playbackPosition + 5000));
  }, [playbackPosition, durationMs, setPlaybackPosition]);

  const cyclePlaybackRate = () => {
    const rates = [0.5, 0.75, 1, 1.25, 1.5, 2];
    const idx = rates.indexOf(playbackRate);
    const next = rates[(idx + 1) % rates.length];
    setPlaybackRate(next);
    if (videoRef.current) {
      videoRef.current.playbackRate = next;
    }
  };

  return (
    <div className="flex items-center gap-3 border-t bg-background px-4 py-2">
      <button
        onClick={() => setIsPlaying(!isPlaying)}
        className="rounded-md p-1.5 hover:bg-secondary"
        title={isPlaying ? "Pause" : "Play"}
      >
        {isPlaying ? <Pause className="h-4 w-4" /> : <Play className="h-4 w-4" />}
      </button>

      <button onClick={handleSkipBack} className="rounded-md p-1.5 hover:bg-secondary" title="Back 5s">
        <SkipBack className="h-4 w-4" />
      </button>

      <button onClick={handleSkipForward} className="rounded-md p-1.5 hover:bg-secondary" title="Forward 5s">
        <SkipForward className="h-4 w-4" />
      </button>

      <span className="text-xs font-mono tabular-nums w-20 text-center">
        {formatTime(playbackPosition)} / {formatTime(durationMs)}
      </span>

      <input
        type="range"
        min={0}
        max={durationMs}
        step={100}
        value={playbackPosition}
        onChange={handleSeek}
        className="flex-1 accent-primary h-1"
      />

      <button
        onClick={cyclePlaybackRate}
        className="rounded-md px-2 py-1 text-xs font-mono hover:bg-secondary min-w-[3rem]"
        title="Playback speed"
      >
        {playbackRate}x
      </button>

      <button
        onClick={() => setIsMuted(!isMuted)}
        className="rounded-md p-1.5 hover:bg-secondary"
        title={isMuted ? "Unmute" : "Mute"}
      >
        {isMuted ? <VolumeX className="h-4 w-4" /> : <Volume2 className="h-4 w-4" />}
      </button>
    </div>
  );
}
