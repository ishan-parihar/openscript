import React from 'react';
import { AbsoluteFill, Sequence, useCurrentFrame, interpolate, spring, useVideoConfig } from 'remotion';

export const HotMotion: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps, durationInFrames } = useVideoConfig();

  const opacity = interpolate(frame, [0, 30], [0, 1], { extrapolateLeft: 'clamp', extrapolateRight: 'clamp' });
  const scale = spring({ frame, fps, from: 0.8, to: 1, config: { damping: 12 } });

  return (
    <AbsoluteFill style={{ backgroundColor: '#020105', justifyContent: 'center', alignItems: 'center' }}>
      <Sequence from={0} durationInFrames={900}>
        <AbsoluteFill style={{ opacity, transform: `scale(${scale})` }}>
          <div style={{ textAlign: 'center' }}>
            <h1 style={{ color: '#EEEDF5', fontSize: 55, fontFamily: 'Plus Jakarta Sans', fontWeight: 600, margin: 0 }}>
              OpenScript
            </h1>
            <p style={{ color: '#9E9CAA', fontSize: 22, fontFamily: 'Plus Jakarta Sans', fontWeight: 600, marginTop: 24 }}>
              AI-Directed Video Editing
            </p>
          </div>
        </AbsoluteFill>
      </Sequence>
    </AbsoluteFill>
  );
};

export default HotMotion;
