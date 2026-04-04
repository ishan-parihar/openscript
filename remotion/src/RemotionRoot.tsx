import { Root } from 'remotion';
import { MainWithBroll } from './compositions/MainWithBroll';
import { TimelineSchema } from './lib/track';

export const RemotionRoot: React.FC = () => {
  return (
    <Root
      composition={[
        {
          id: 'MainWithBroll',
          component: MainWithBroll,
          durationInFrames: 900, // Will be overridden by props.meta
          fps: 30,
          width: 1080,
          height: 1920,
          defaultProps: {
            timeline: {
              meta: { fps: 30, width: 1080, height: 1920, durationMs: 30000 },
              sources: { main: '', brolls: [] },
              track: [],
            },
          },
          schemaValidator: ({ timeline }) => {
            TimelineSchema.parse(timeline);
          },
        },
      ]}
    />
  );
};

export default RemotionRoot;
