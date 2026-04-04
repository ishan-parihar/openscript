import { z } from 'zod';

// Timeline event types
export const TransitionSchema = z.object({
  in: z.number().default(6),  // frames
  out: z.number().default(6),
});

export const BrollClipSchema = z.object({
  id: z.string(),
  src: z.string(),  // URL or absolute path
  durationMs: z.number().optional(),
  width: z.number().optional(),
  height: z.number().optional(),
});

export const TimelineEventSchema = z.union([
  z.object({
    type: z.literal('video'),
    role: z.literal('main'),
    startMs: z.number(),
    endMs: z.number(),
  }),
  z.object({
    type: z.literal('broll'),
    id: z.string(),
    startMs: z.number(),
    endMs: z.number(),
    transition: TransitionSchema.optional(),
  }),
]);

export const TimelineMetaSchema = z.object({
  fps: z.number(),
  width: z.number().default(1080),
  height: z.number().default(1920),
  durationMs: z.number(),
});

export const TimelineSchema = z.object({
  meta: TimelineMetaSchema,
  sources: z.object({
    main: z.string(),
    brolls: z.array(BrollClipSchema),
  }),
  track: z.array(TimelineEventSchema),
});

export type Timeline = z.infer<typeof TimelineSchema>;
export type TimelineEvent = z.infer<typeof TimelineEventSchema>;
export type BrollClip = z.infer<typeof BrollClipSchema>;
export type Transition = z.infer<typeof TransitionSchema>;

// Helper: ms to frames
export const msToFrames = (ms: number, fps: number): number => {
  return Math.round((ms * fps) / 1000);
};

// Helper: validate timeline
export const validateTimeline = (data: unknown): Timeline => {
  return TimelineSchema.parse(data);
};
