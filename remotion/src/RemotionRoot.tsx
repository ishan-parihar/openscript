import { registerRoot, Composition } from 'remotion';
import React from 'react';
import { MainWithBroll } from './compositions/MainWithBroll';

const RemotionRoot: React.FC = () => {
  return (
    <>
      <Composition
        id="MainWithBroll"
        component={MainWithBroll as unknown as React.FC<Record<string, unknown>>}
        durationInFrames={900}
        fps={30}
        width={1080}
        height={1920}
        defaultProps={{
          timeline: {
            meta: { fps: 30, width: 1080, height: 1920, durationMs: 30000 },
            sources: { main: '', brolls: [] },
            track: [],
          },
        }}
      />
    </>
  );
};

registerRoot(RemotionRoot);

export default RemotionRoot;
