import { useEffect, useRef } from "react";

interface WaveformDisplayProps {
  src?: string;
  height?: number;
  color?: string;
}

export function WaveformDisplay({ src, height = 40, color = "#3b82f6" }: WaveformDisplayProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;

    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const dpr = window.devicePixelRatio || 1;
    const rect = canvas.getBoundingClientRect();
    canvas.width = rect.width * dpr;
    canvas.height = rect.height * dpr;
    ctx.scale(dpr, dpr);

    const width = rect.width;
    const barWidth = 2;
    const barGap = 1;
    const barCount = Math.floor(width / (barWidth + barGap));

    ctx.clearRect(0, 0, width, height);

    // Generate a synthetic waveform pattern
    const bars: number[] = [];
    let seed = src ? hashCode(src) : 42;
    for (let i = 0; i < barCount; i++) {
      seed = (seed * 1664525 + 1013904223) & 0x7fffffff;
      const normalized = (seed / 0x7fffffff) * 2 - 1;
      // Create a more realistic waveform envelope (louder in middle, quieter at edges)
      const envelope = Math.sin((i / barCount) * Math.PI) * 0.6 + 0.4;
      bars.push(Math.abs(normalized) * envelope);
    }

    // Draw bars
    const centerY = height / 2;
    for (let i = 0; i < bars.length; i++) {
      const x = i * (barWidth + barGap);
      const barHeight = bars[i] * height * 0.85;
      const alpha = 0.4 + bars[i] * 0.6;

      ctx.fillStyle = color + Math.round(alpha * 255).toString(16).padStart(2, "0");
      ctx.fillRect(x, centerY - barHeight / 2, barWidth, barHeight);
    }
  }, [src, height, color]);

  return (
    <canvas
      ref={canvasRef}
      className="w-full"
      style={{ height: `${height}px` }}
    />
  );
}

function hashCode(str: string): number {
  let hash = 0;
  for (let i = 0; i < str.length; i++) {
    const char = str.charCodeAt(i);
    hash = (hash << 5) - hash + char;
    hash |= 0;
  }
  return Math.abs(hash);
}
