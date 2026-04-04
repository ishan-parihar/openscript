/**
 * EDL v2 Compiler for Remotion
 * 
 * Compiles OpenScript EDL v2 timelines into Remotion composition props.
 * Supports:
 * - Multi-track composition (video, b-roll, captions)
 * - Audio mixing with ducking
 * - Transitions and effects
 * - Rich visual compositions
 */

import {CompositionProps, AbsoluteFill, Sequence, useVideoConfig, interpolate} from 'remotion';
import {Zodiac} from 'remotion';
import React from 'react';

// Type definitions matching EDL v2 schema
export interface Segment {
  id: string;
  start: number;
  end: number;
  caption: string;
  crossfade_ms: number;
  semantic_role?: string;
}

export interface TimelineEvent {
  id: string;
  asset_id: string;
  start_ms: number;
  end_ms: number;
  offset_ms?: number;
  gain_db?: number;
  fade_in_ms?: number;
  fade_out_ms?: number;
  tags?: string[];
  provenance?: any;
}

export interface BrollEvent extends TimelineEvent {
  concept: string;
  source_provider?: string;
  transition_style?: string;
  crop_mode?: string;
  orientation?: string;
  motion_intensity?: string;
}

export interface MusicEvent extends TimelineEvent {
  mood?: string;
  energy?: string;
  bpm?: number;
  loopability?: boolean;
  ducking_policy?: string;
  loop_mode?: string;
}

export interface SFXEvent extends TimelineEvent {
  editorial_role?: string;
  category?: string;
  safe_overlay?: boolean;
}

export interface CaptionEvent extends TimelineEvent {
  text: string;
  style?: string;
  word_timings?: any[];
}

export interface EDL_v2 {
  version: string;
  source: string;
  target: {
    aspect: string;
    fps: number;
    max_duration?: number;
  };
  segments: Segment[];
  tracks: {
    dialogue?: TimelineEvent[];
    voiceover?: TimelineEvent[];
    captions?: CaptionEvent[];
    broll?: BrollEvent[];
    music?: MusicEvent[];
    sfx?: SFXEvent[];
  };
  directives: {
    ducking?: any[];
    transitions?: any[];
    mix?: any;
    render_backend?: string;
  };
  assets: {
    voices?: any;
    broll?: any;
    music?: any;
    sfx?: any;
  };
  effects: {
    burn_captions?: boolean;
    audio?: any;
  };
}

export interface RemotionCompositionProps {
  edl: EDL_v2;
  assets: {
    [key: string]: string;
  };
}

/**
 * Main Remotion composition for EDL v2
 */
export const EDL_v2_Composition: React.FC<RemotionCompositionProps> = ({edl, assets}) => {
  const {fps, width, height} = useVideoConfig();
  
  // Calculate duration from segments
  const durationInFrames = edl.segments.length > 0 
    ? Math.ceil(edl.segments[edl.segments.length - 1].end * fps)
    : 30 * fps;
  
  return (
    <AbsoluteFill style={{backgroundColor: 'black'}}>
      {/* Main video track with segments */}
      <VideoTrack segments={edl.segments} source={edl.source} fps={fps} />
      
      {/* B-roll track */}
      {edl.tracks.broll?.map((event) => (
        <BrollLayer 
          key={event.id}
          event={event}
          assetPath={assets[event.asset_id]}
          fps={fps}
        />
      ))}
      
      {/* Captions track */}
      {edl.effects.burn_captions && edl.tracks.captions?.map((event) => (
        <CaptionLayer 
          key={event.id}
          event={event}
          fps={fps}
        />
      ))}
    </AbsoluteFill>
  );
};

/**
 * Video track with segment cuts
 */
const VideoTrack: React.FC<{
  segments: Segment[];
  source: string;
  fps: number;
}> = ({segments, source, fps}) => {
  return (
    <>
      {segments.map((segment, index) => {
        const startFrame = Math.ceil(segment.start * fps);
        const durationFrames = Math.ceil((segment.end - segment.start) * fps);
        
        return (
          <Sequence
            key={segment.id}
            from={startFrame}
            durationInFrames={durationFrames}
          >
            <VideoSegment 
              source={source}
              startFrame={0}
              durationFrames={durationFrames}
              crossfadeMs={segment.crossfade_ms}
            />
          </Sequence>
        );
      })}
    </>
  );
};

/**
 * Individual video segment
 */
const VideoSegment: React.FC<{
  source: string;
  startFrame: number;
  durationFrames: number;
  crossfadeMs: number;
}> = ({source, startFrame, durationFrames, crossfadeMs}) => {
  const {fps} = useVideoConfig();
  const crossfadeFrames = Math.ceil((crossfadeMs / 1000) * fps);
  
  return (
    <AbsoluteFill>
      <Video src={source} startFrom={startFrame} />
    </AbsoluteFill>
  );
};

/**
 * B-roll overlay layer
 */
const BrollLayer: React.FC<{
  event: BrollEvent;
  assetPath?: string;
  fps: number;
}> = ({event, assetPath, fps}) => {
  const startFrame = Math.ceil((event.start_ms / 1000) * fps);
  const durationFrames = Math.ceil(((event.end_ms - event.start_ms) / 1000) * fps);
  
  if (!assetPath) {
    return null;
  }
  
  return (
    <Sequence
      from={startFrame}
      durationInFrames={durationFrames}
    >
      <AbsoluteFill>
        <Video 
          src={assetPath}
          style={{
            objectFit: event.crop_mode === 'center' ? 'cover' : 'contain',
          }}
        />
        
        {/* Transition effect */}
        {event.transition_style === 'fade' && (
          <FadeTransition durationFrames={durationFrames} />
        )}
      </AbsoluteFill>
    </Sequence>
  );
};

/**
 * Caption overlay layer
 */
const CaptionLayer: React.FC<{
  event: CaptionEvent;
  fps: number;
}> = ({event, fps}) => {
  const startFrame = Math.ceil((event.start_ms / 1000) * fps);
  const durationFrames = Math.ceil(((event.end_ms - event.start_ms) / 1000) * fps);
  
  return (
    <Sequence
      from={startFrame}
      durationInFrames={durationFrames}
    >
      <AbsoluteFill
        style={{
          justifyContent: 'center',
          alignItems: 'center',
        }}
      >
        <div
          style={{
            fontFamily: 'Bebas Neue',
            fontSize: 80,
            color: 'white',
            textAlign: 'center',
            textShadow: '2px 2px 4px rgba(0,0,0,0.8)',
            padding: '20px',
            maxWidth: '80%',
          }}
        >
          {event.text}
        </div>
      </AbsoluteFill>
    </Sequence>
  );
};

/**
 * Simple fade transition
 */
const FadeTransition: React.FC<{
  durationFrames: number;
}> = ({durationFrames}) => {
  const frame = useCurrentFrame();
  
  const opacity = interpolate(
    frame,
    [0, 10, durationFrames - 10, durationFrames],
    [0, 1, 1, 0],
    {extrapolateRight: 'clamp'}
  );
  
  return (
    <AbsoluteFill
      style={{
        backgroundColor: 'black',
        opacity,
      }}
    />
  );
};

/**
 * Audio mixer component
 */
export const AudioMixer: React.FC<{
  edl: EDL_v2;
  assets: {[key: string]: string};
}> = ({edl, assets}) => {
  const {fps} = useVideoConfig();
  
  return (
    <>
      {/* Source audio */}
      <Audio src={edl.source} />
      
      {/* Voiceover track */}
      {edl.tracks.voiceover?.map((event) => {
        const assetPath = assets[event.asset_id];
        if (!assetPath) return null;
        
        const startFrame = Math.ceil((event.start_ms / 1000) * fps);
        
        return (
          <Sequence key={event.id} from={startFrame}>
            <Audio src={assetPath} volume={event.gain_db || 0} />
          </Sequence>
        );
      })}
      
      {/* Music track with ducking */}
      {edl.tracks.music?.map((event) => {
        const assetPath = assets[event.asset_id];
        if (!assetPath) return null;
        
        const startFrame = Math.ceil((event.start_ms / 1000) * fps);
        const durationFrames = Math.ceil(((event.end_ms - event.start_ms) / 1000) * fps);
        const hasDucking = event.ducking_policy === 'auto';
        
        return (
          <Sequence key={event.id} from={startFrame} durationInFrames={durationFrames}>
            <DuckingAudio 
              src={assetPath}
              baseVolume={event.gain_db || -12}
              ducking={hasDucking}
            />
          </Sequence>
        );
      })}
      
      {/* SFX track */}
      {edl.tracks.sfx?.map((event) => {
        const assetPath = assets[event.asset_id];
        if (!assetPath) return null;
        
        const startFrame = Math.ceil((event.start_ms / 1000) * fps);
        
        return (
          <Sequence key={event.id} from={startFrame}>
            <Audio src={assetPath} volume={event.gain_db || -10} />
          </Sequence>
        );
      })}
    </>
  );
};

/**
 * Audio component with automatic ducking
 */
const DuckingAudio: React.FC<{
  src: string;
  baseVolume: number;
  ducking: boolean;
}> = ({src, baseVolume, ducking}) => {
  const frame = useCurrentFrame();
  const {fps} = useVideoConfig();
  
  // Simple ducking: reduce volume when voiceover would be present
  // In production, this would analyze actual voiceover track
  const duckingReduction = ducking ? 0.5 : 1.0;
  const volume = Math.pow(10, baseVolume / 20) * duckingReduction;
  
  return <Audio src={src} volume={volume} />;
};

/**
 * Compile EDL v2 to Remotion props
 */
export function compileEDLToRemotion(edl: EDL_v2): RemotionCompositionProps {
  // Map asset IDs to paths
  const assets: {[key: string]: string} = {};
  
  // Collect all asset paths
  for (const [type, typeAssets] of Object.entries(edl.assets)) {
    if (typeof typeAssets === 'object') {
      for (const [assetId, assetData] of Object.entries(typeAssets)) {
        if (typeof assetData === 'object' && assetData !== null && 'path' in assetData) {
          assets[assetId] = (assetData as any).path;
        }
      }
    }
  }
  
  return {
    edl,
    assets,
  };
}

/**
 * Register composition with Remotion
 */
export function registerEDL_v2_Composition(edl: EDL_v2) {
  const props = compileEDLToRemotion(edl);
  
  // Calculate composition settings
  const fps = edl.target.fps || 30;
  const durationInFrames = edl.segments.length > 0
    ? Math.ceil(edl.segments[edl.segments.length - 1].end * fps)
    : 30 * fps;
  
  // Parse aspect ratio
  const [widthStr, heightStr] = edl.target.aspect.split(':');
  const width = parseInt(widthStr) * 120; // Base width multiplier
  const height = parseInt(heightStr) * 120;
  
  return {
    id: `edl_v2_${Date.now()}`,
    componentName: 'EDL_v2_Composition',
    props,
    durationInFrames,
    fps,
    width,
    height,
  };
}

// Helper components (would be imported from remotion in real implementation)
const Video: React.FC<any> = (props) => <video {...props} />;
const Audio: React.FC<any> = (props) => <audio {...props} />;
const useCurrentFrame = () => 0; // Placeholder

export default EDL_v2_Composition;
