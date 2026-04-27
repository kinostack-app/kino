import {
  Keyboard,
  Maximize,
  Minimize,
  Pause,
  PictureInPicture,
  Play,
  SkipBack,
  SkipForward,
} from 'lucide-react';
import type { RefObject } from 'react';
import { cn } from '@/lib/utils';
import type { VideoSource } from '../types';
import { formatTime } from '../types';
import { AudioTrackMenu } from './AudioTrackMenu';
import { CastButton } from './CastButton';
import { ScrubBar } from './ScrubBar';
import { SpeedMenu } from './SpeedMenu';
import { SubtitleMenu } from './SubtitleMenu';
import { VolumeControl } from './VolumeControl';

/**
 * Bottom-of-player controls. Seek bar on top, button row
 * below split left/right. Consumers own state — this is
 * pure presentation + wiring.
 */
export function ControlBar({
  source,
  videoRef,
  hlsOffsetRef,
  playing,
  currentTime,
  duration,
  buffered,
  seekExtraLayer,
  volume,
  muted,
  playbackRate,
  isFullscreen,
  isPip,
  pipSupported,
  subtitleStreamIndex,
  setSubtitleStreamIndex,
  userHasPickedSubtitleRef,
  onRequestPause,
  onRequestResume,
  onSeek,
  onPlaybackRateChange,
  onMuteToggle,
  onVolumeChange,
  onToggleFullscreen,
  onTogglePip,
  onOpenShortcuts,
}: {
  source: VideoSource | null;
  videoRef: RefObject<HTMLVideoElement | null>;
  hlsOffsetRef: RefObject<number>;
  playing: boolean;
  currentTime: number;
  duration: number;
  buffered: number;
  seekExtraLayer?: React.ReactNode;
  volume: number;
  muted: boolean;
  playbackRate: number;
  isFullscreen: boolean;
  isPip: boolean;
  pipSupported: boolean;
  subtitleStreamIndex: number | null;
  setSubtitleStreamIndex: (idx: number | null) => void;
  userHasPickedSubtitleRef: RefObject<boolean>;
  onRequestPause: () => boolean;
  onRequestResume: () => void;
  onSeek: (sourceSec: number) => void;
  onPlaybackRateChange: (rate: number) => void;
  onMuteToggle: () => void;
  onVolumeChange: (v: number) => void;
  onToggleFullscreen: () => void;
  onTogglePip: () => void;
  onOpenShortcuts: () => void;
}) {
  return (
    <div className="bg-gradient-to-t from-black/80 via-black/30 to-transparent px-4 pb-4 pt-12">
      <ScrubBar
        source={source}
        currentTime={currentTime}
        duration={duration}
        buffered={buffered}
        seekExtraLayer={seekExtraLayer}
        onSeek={onSeek}
      />
      <div className="flex items-center justify-between gap-4">
        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={() => {
              const v = videoRef.current;
              if (!v) return;
              if (v.paused) void v.play();
              else v.pause();
            }}
            className="w-9 h-9 grid place-items-center hover:bg-white/10 rounded-lg transition"
            aria-label={playing ? 'Pause' : 'Play'}
          >
            {playing ? (
              <Pause size={20} fill="white" />
            ) : (
              <Play size={20} fill="white" className="ml-0.5" />
            )}
          </button>
          <button
            type="button"
            onClick={() => onSeek(Math.max(0, currentTime - 10))}
            className="w-9 h-9 grid place-items-center hover:bg-white/10 rounded-lg transition"
            aria-label="Skip back 10 seconds"
          >
            <SkipBack size={18} />
          </button>
          <button
            type="button"
            onClick={() => onSeek(Math.min(duration || 0, currentTime + 10))}
            className="w-9 h-9 grid place-items-center hover:bg-white/10 rounded-lg transition"
            aria-label="Skip forward 10 seconds"
          >
            <SkipForward size={18} />
          </button>
          <VolumeControl
            volume={volume}
            muted={muted}
            onVolumeChange={onVolumeChange}
            onMuteToggle={onMuteToggle}
          />
          <span className="text-xs text-white/60 ml-2 tabular-nums">
            {formatTime(currentTime)} / {formatTime(duration)}
          </span>
        </div>

        <div className="flex items-center gap-1">
          <SpeedMenu playbackRate={playbackRate} onChange={onPlaybackRateChange} />
          <AudioTrackMenu source={source} />
          <SubtitleMenu
            source={source}
            subtitleStreamIndex={subtitleStreamIndex}
            setSubtitleStreamIndex={setSubtitleStreamIndex}
            userHasPickedRef={userHasPickedSubtitleRef}
            onRequestPause={onRequestPause}
            onRequestResume={onRequestResume}
          />
          <CastButton source={source} videoRef={videoRef} hlsOffsetRef={hlsOffsetRef} />
          {pipSupported && (
            <button
              type="button"
              onClick={onTogglePip}
              aria-label={isPip ? 'Exit picture-in-picture' : 'Picture-in-picture'}
              className={cn(
                'w-9 h-9 grid place-items-center rounded-lg transition',
                isPip ? 'bg-white/15 text-white' : 'hover:bg-white/10 text-white/70'
              )}
            >
              <PictureInPicture size={18} />
            </button>
          )}
          <button
            type="button"
            onClick={onOpenShortcuts}
            aria-label="Keyboard shortcuts"
            className="w-9 h-9 grid place-items-center hover:bg-white/10 rounded-lg transition text-white/70 hidden sm:grid"
          >
            <Keyboard size={18} />
          </button>
          <button
            type="button"
            onClick={onToggleFullscreen}
            className="w-9 h-9 grid place-items-center hover:bg-white/10 rounded-lg transition"
            aria-label={isFullscreen ? 'Exit fullscreen' : 'Fullscreen'}
          >
            {isFullscreen ? <Minimize size={18} /> : <Maximize size={18} />}
          </button>
        </div>
      </div>
    </div>
  );
}
