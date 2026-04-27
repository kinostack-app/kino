/**
 * Add-list modal. Two-phase flow:
 *   1. User pastes URL → POST with confirm=false → backend returns a
 *      preview (title, description, count, item_type).
 *   2. User confirms → POST with confirm=true → list created.
 *
 * MDBList source with no API key configured produces a typed error
 * from the backend — we surface a link to the Settings integrations
 * page instead of a generic "failed" toast.
 */

import { useMutation } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { Loader2, X } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { createList } from '@/api/generated/sdk.gen';
import type { CreateListResponse, ListPreview } from '@/api/generated/types.gen';
import { useModalA11y } from '@/hooks/useModalA11y';
import { cn } from '@/lib/utils';

interface Props {
  open: boolean;
  onClose: () => void;
  onCreated: () => void;
}

export function AddListModal({ open, onClose, onCreated }: Props) {
  const [url, setUrl] = useState('');
  const [preview, setPreview] = useState<ListPreview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const overlayRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const { titleId, dialogProps } = useModalA11y({
    open,
    onClose,
    containerRef: contentRef,
    initialFocusRef: inputRef,
  });

  // Reset on close so re-opening isn't sticky.
  useEffect(() => {
    if (open) return;
    setUrl('');
    setPreview(null);
    setError(null);
  }, [open]);

  const previewMutation = useMutation({
    mutationFn: async (u: string) => {
      // hey-api returns { data?, error? } instead of throwing on 4xx/5xx,
      // so we need to promote error bodies ourselves for onError to fire.
      const res = await createList({ body: { url: u } });
      if (res.error) throw res.error;
      if (!res.data) throw new Error('Empty response from server');
      return res.data as CreateListResponse;
    },
    onSuccess: (data) => {
      if (data.preview) setPreview(data.preview);
      setError(null);
    },
    onError: (e: unknown) => {
      setError(extractErrorMessage(e));
    },
  });

  const createMutation = useMutation({
    mutationFn: async () => {
      const res = await createList({ body: { url, confirm: true } });
      if (res.error) throw res.error;
      if (!res.data) throw new Error('Empty response from server');
      return res.data as CreateListResponse;
    },
    onSuccess: () => {
      onCreated();
      onClose();
    },
    onError: (e: unknown) => {
      setError(extractErrorMessage(e));
    },
  });

  if (!open) return null;

  const handleSubmit = () => {
    if (!url.trim()) return;
    setError(null);
    previewMutation.mutate(url.trim());
  };

  return createPortal(
    // biome-ignore lint/a11y/noStaticElementInteractions: backdrop click is a visual dismiss; keyboard dismissal is handled by useModalA11y
    <div
      ref={overlayRef}
      role="presentation"
      className="fixed inset-0 z-[60] flex items-center justify-center bg-black/60 backdrop-blur-sm p-4"
      onClick={(e) => {
        if (e.target === overlayRef.current) onClose();
      }}
    >
      <div
        ref={contentRef}
        className="bg-[var(--bg-secondary)] rounded-xl max-w-md w-full border border-white/10 shadow-2xl"
        {...dialogProps}
      >
        <div className="flex items-center justify-between px-6 py-4 border-b border-white/5">
          <h2 id={titleId} className="text-base font-semibold">
            Add a list
          </h2>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close"
            className="p-1 rounded hover:bg-white/5 text-[var(--text-muted)]"
          >
            <X size={16} />
          </button>
        </div>

        <div className="p-6">
          {!preview && (
            <>
              <label
                htmlFor="list-url"
                className="block text-xs font-semibold text-[var(--text-muted)] uppercase tracking-wider mb-2"
              >
                List URL
              </label>
              <input
                ref={inputRef}
                id="list-url"
                type="url"
                value={url}
                onChange={(e) => setUrl(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') handleSubmit();
                }}
                placeholder="https://mdblist.com/lists/… or trakt.tv/users/…"
                className="w-full px-3 py-2 bg-[var(--bg-card)] border border-white/10 rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-[var(--accent)]/40"
              />
              <p className="mt-2 text-xs text-[var(--text-muted)]">
                Supports MDBList, TMDB lists, and Trakt lists.
              </p>
            </>
          )}

          {preview && (
            <div className="space-y-3">
              <div>
                <p className="text-xs text-[var(--text-muted)]">
                  {preview.source_type.replace('_', ' ')}
                </p>
                <h3 className="mt-1 text-base font-semibold text-white">{preview.title}</h3>
                {preview.description && (
                  <p className="mt-1 text-xs text-[var(--text-secondary)] line-clamp-3">
                    {preview.description}
                  </p>
                )}
              </div>
              <div className="text-sm">
                <span className="tabular-nums text-white font-semibold">{preview.item_count}</span>
                <span className="text-[var(--text-muted)] ml-1">
                  {preview.item_type === 'mixed' ? 'items' : preview.item_type}
                </span>
              </div>
              <p className="text-xs text-[var(--text-muted)]">
                Items land on the list detail page — click any one to add it to your library.
              </p>
            </div>
          )}

          {error && (
            <div className="mt-3 rounded-lg bg-red-500/10 border border-red-500/20 p-3 text-xs text-red-300">
              {error}
              {error.toLowerCase().includes('mdblist') && (
                <>
                  {' '}
                  <Link
                    to="/settings/integrations"
                    onClick={onClose}
                    className="underline hover:text-white"
                  >
                    Open integrations settings
                  </Link>
                  .
                </>
              )}
            </div>
          )}
        </div>

        <div className="px-6 py-4 bg-[var(--bg-card)]/60 rounded-b-xl flex items-center justify-end gap-2">
          {!preview && (
            <>
              <button
                type="button"
                onClick={onClose}
                className="px-3 py-2 rounded-lg text-sm text-[var(--text-secondary)] hover:bg-white/5"
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={handleSubmit}
                disabled={!url.trim() || previewMutation.isPending}
                className={cn(
                  'inline-flex items-center gap-2 px-3 py-2 rounded-lg bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white text-sm font-semibold disabled:opacity-50'
                )}
              >
                {previewMutation.isPending && (
                  <Loader2 size={14} className="motion-safe:animate-spin" />
                )}
                Preview
              </button>
            </>
          )}
          {preview && (
            <>
              <button
                type="button"
                onClick={() => {
                  setPreview(null);
                }}
                className="px-3 py-2 rounded-lg text-sm text-[var(--text-secondary)] hover:bg-white/5"
              >
                Back
              </button>
              <button
                type="button"
                onClick={() => createMutation.mutate()}
                disabled={createMutation.isPending}
                className="inline-flex items-center gap-2 px-3 py-2 rounded-lg bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white text-sm font-semibold disabled:opacity-50"
              >
                {createMutation.isPending && (
                  <Loader2 size={14} className="motion-safe:animate-spin" />
                )}
                Add
              </button>
            </>
          )}
        </div>
      </div>
    </div>,
    document.body
  );
}

function extractErrorMessage(e: unknown): string {
  // hey-api thrown error bodies match our AppError shape:
  //   { error: { code, message } }
  // We also fall through to `body.error.message` for any future
  // helpers that wrap the response with a `body` envelope.
  if (e && typeof e === 'object') {
    const top = e as { error?: { message?: string }; body?: { error?: { message?: string } } };
    if (top.error?.message) return top.error.message;
    if (top.body?.error?.message) return top.body.error.message;
  }
  if (e instanceof Error) return e.message;
  return 'Something went wrong.';
}
