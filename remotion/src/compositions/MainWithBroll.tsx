import React, { useMemo } from 'react';
import { AbsoluteFill, Sequence, Video, Audio, interpolate, useCurrentFrame } from 'remotion';
import { Timeline, TimelineEvent, msToFrames } from '../lib/track';

interface MainWithBrollProps {
  timeline: Timeline;
}

// Crossfade transition component
const CrossFadeVideo: React.FC<{
  src: string;
  from: number;
  to: number;
  transitionIn?: number;
  transitionOut?: number;
  muted?: boolean;
}> = ({ src, from, to, transitionIn = 6, transitionOut = 6, muted = true }) => {
  const frame = useCurrentFrame();
  const duration = to - from;
  
  const opacity = useMemo(() => {
    if (frame < from + transitionIn) {
      return interpolate(frame, [from, from + transitionIn], [0, 1], { extrapolateRight: 'clamp' });
    }
    if (frame > to - transitionOut) {
      return interpolate(frame, [to - transitionOut, to], [1, 0], { extrapolateLeft: 'clamp' });
    }
    return 1;
  }, [frame, from, to, transitionIn, transitionOut]);

  return (
    <AbsoluteFill style={{ opacity }}>
      <Video
        src={src}
        startFrom={from}
        endAt={to}
        muted={muted}
        style={{
          width: '100%',
          height: '100%',
          objectFit: 'cover',
        }}
      />
    </AbsoluteFill>
  );
};

// Main video layer (shows when no b-roll)
const MainVideoLayer: React.FC<{ src: string; events: TimelineEvent[]; fps: number }> = ({ src, events, fps }) => {
  return (
    <AbsoluteFill>
      {events.map((event, i) => {
        if (event.type !== 'video') return null;
        const from = msToFrames(event.startMs, fps);
        const to = msToFrames(event.endMs, fps);
        return (
          <Sequence key={i} from={from} durationInFrames={to - from}>
            <Video
              src={src}
              startFrom={from}
              endAt={to}
              muted
              style={{
                width: '100%',
                height: '100%',
                objectFit: 'cover',
              }}
            />
          </Sequence>
        );
      })}
    </AbsoluteFill>
  );
};

// B-roll layer
const BrollLayer: React.FC<{
  brolls: { id: string; src: string }[];
  events: TimelineEvent[];
  fps: number;
}> = ({ brolls, events, fps }) => {
  const brollMap = useMemo(() => {
    const map = new Map<string, string>();
    brolls.forEach((b) => map.set(b.id, b.src));
    return map;
  }, [brolls]);

  return (
    <AbsoluteFill>
      {events.map((event, i) => {
        if (event.type !== 'broll') return null;
        const src = brollMap.get(event.id);
        if (!src) return null;

        const from = msToFrames(event.startMs, fps);
        const to = msToFrames(event.endMs, fps);
        const transition = event.transition || { in: 6, out: 6 };

        return (
          <Sequence key={i} from={from} durationInFrames={to - from}>
            <CrossFadeVideo
              src={src}
              from={0}
              to={to - from}
              transitionIn={transition.in}
              transitionOut={transition.out}
              muted
            />
          </Sequence>
        );
      })}
    </AbsoluteFill>
  );
};

// Main composition
export const MainWithBroll: React.FC<MainWithBrollProps> = ({ timeline }) => {
  const { meta, sources, track } = timeline;
  const durationInFrames = msToFrames(meta.durationMs, meta.fps);

  const videoEvents = useMemo(
    () => track.filter((e): e is Extract<TimelineEvent, { type: 'video' }> => e.type === 'video'),
    [track]
  );

  const brollEvents = useMemo(
    () => track.filter((e): e is Extract<TimelineEvent, { type: 'broll' }> => e.type === 'broll'),
    [track]
  );

  return (
    <AbsoluteFill style={{ backgroundColor: '#000' }}>
      {/* Audio: always from main video */}
      {sources.main && <Audio src={sources.main} />}

      {/* Video layers */}
      <MainVideoLayer src={sources.main} events={videoEvents} fps={meta.fps} />
      <BrollLayer brolls={sources.brolls} events={brollEvents} fps={meta.fps} />
    </AbsoluteFill>
  );
};

export default MainWithBroll;
