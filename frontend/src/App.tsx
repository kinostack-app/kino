import { QueryClient, QueryClientProvider, useQuery } from '@tanstack/react-query';
import { RouterProvider } from '@tanstack/react-router';
import { useEffect, useState } from 'react';
import { Toaster } from 'sonner';
import { getStatus, updateConfig } from '@/api/generated/sdk.gen';
import { AuthGate } from '@/components/auth/AuthGate';
import { AppErrorBoundary } from '@/components/ErrorBoundary';
import { OfflineShell } from '@/components/OfflineShell';
import { ReconnectingBanner } from '@/components/ReconnectingBanner';
import { SetupWizard } from '@/components/SetupWizard';
import { router } from '@/router';
import { useConnectionStore } from '@/state/connection';
import type { InvalidationRule } from '@/state/invalidation';
import { connectWebSocket, disconnectWebSocket } from '@/state/websocket';
import '@/lib/api';
import { clientLog, initClientLogger } from '@/lib/clientLogger';

/** App-level status (first-time-setup flag + warning list) —
 *  refreshed whenever anything that can flip a warning changes. */
const STATUS_INVALIDATED_BY: InvalidationRule[] = [
  'health_warning',
  'health_recovered',
  'indexer_changed',
  'config_changed',
  'download_started',
  'download_complete',
  'download_failed',
  'download_cancelled',
  'ffmpeg_download_completed',
  'ffmpeg_download_failed',
  'ip_leak_detected',
];

// Ship uncaught errors + failed requests to /api/v1/client-logs so the
// in-app log viewer shows frontend problems alongside backend ones.
initClientLogger();

// Route-change breadcrumbs — one INFO line per navigation so the log
// viewer shows what the user was doing when something broke.
router.subscribe('onResolved', ({ toLocation }) => {
  clientLog.info(`navigated to ${toLocation.pathname}`);
});

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 5 * 60 * 1000,
      retry: 1,
    },
  },
});

function WebSocketProvider() {
  useEffect(() => {
    connectWebSocket(queryClient);
    return () => disconnectWebSocket();
  }, []);
  return null;
}

function AppContent() {
  const [setupComplete, setSetupComplete] = useState(false);
  const [setupNeeded, setSetupNeeded] = useState<boolean | null>(() => {
    // Persist wizard state across refreshes
    const stored = sessionStorage.getItem('kino-setup-in-progress');
    return stored === 'true' ? true : null;
  });

  const { data: status, isError: statusErrored } = useQuery({
    queryKey: ['kino', 'status'],
    queryFn: async () => {
      const { data } = await getStatus();
      // hey-api returns `{data: undefined}` on fetch error rather
      // than throwing. Without this check the query reports success
      // with `data: undefined` and downstream logic stalls on null-
      // checks forever — the infinite-spinner-on-backend-down bug.
      if (!data) throw new Error('status response empty');
      return data as {
        status: string;
        // `first_time_setup` is true only when core config (TMDB key +
        // paths) is missing. Runtime issues like "no indexers" surface
        // via the HealthBanner instead — they don't re-trigger the
        // wizard once initial setup is done.
        first_time_setup: boolean;
        setup_required: boolean;
        warnings: { message: string; route?: string | null }[];
      };
    },
    staleTime: 30_000,
    // Higher retry budget on the mount-blocking query so a backend
    // that's a few seconds into a restart doesn't kick the offline
    // shell in the user's face immediately. Exponential backoff
    // capped at 4 s to keep the transition predictable.
    retry: 3,
    retryDelay: (attempt) => Math.min(500 * 2 ** attempt, 4_000),
    // No polling — meta-driven invalidation fires on health /
    // indexer / config / download-lifecycle events.
    meta: { invalidatedBy: STATUS_INVALIDATED_BY },
  });

  // Set setupNeeded on initial load, persist to sessionStorage
  useEffect(() => {
    if (status && setupNeeded === null) {
      const needed = status.first_time_setup;
      setSetupNeeded(needed);
      if (needed) {
        sessionStorage.setItem('kino-setup-in-progress', 'true');
      }
    }
  }, [status, setupNeeded]);

  const showSetup = setupNeeded === true && !setupComplete;
  const connectionPhase = useConnectionStore((s) => s.phase);

  // Connection state wins over setup state. If the backend's gone
  // away entirely (fetch interceptor flipped us to `offline` after
  // 30 s of failures) or the status query ran out of retries, take
  // over the whole viewport with the OfflineShell. The user can't
  // do anything useful until the server's back.
  if (connectionPhase === 'offline' || (statusErrored && setupNeeded === null)) {
    return <OfflineShell />;
  }

  // Show loading while we check if setup is needed. Only reached
  // when the status query is in-flight AND hasn't errored yet —
  // the condition above catches the "errored before setup probed"
  // case that used to spin forever.
  if (setupNeeded === null) {
    return (
      <div className="min-h-screen bg-[var(--bg-primary)] flex items-center justify-center">
        <div className="w-6 h-6 border-2 border-white/20 border-t-white rounded-full animate-spin" />
      </div>
    );
  }

  if (showSetup) {
    return (
      <SetupWizard
        onComplete={() => {
          setSetupComplete(true);
          setSetupNeeded(false);
          sessionStorage.removeItem('kino-setup-in-progress');
          queryClient.invalidateQueries({ queryKey: ['kino', 'status'] });
        }}
        onSave={async (config) => {
          await updateConfig({ body: config });
          queryClient.invalidateQueries({ queryKey: ['kino', 'status'] });
        }}
      />
    );
  }

  return (
    <>
      <WebSocketProvider />
      <ReconnectingBanner />
      <RouterProvider router={router} />
      <Toaster
        theme="dark"
        position="bottom-right"
        // `unstyled` strips Sonner's default wrapper (its own
        // background, rounded corners, border, padding) so the
        // `kinoToast.*` cards are the *only* visible surface.
        // Without this, Sonner's outer div bled through at the
        // rounded corners, and its inner padding pushed our card's
        // content off-edge. See components/kino-toast.tsx for the
        // actual card markup.
        toastOptions={{ unstyled: true }}
      />
    </>
  );
}

export function App() {
  return (
    <AppErrorBoundary>
      <QueryClientProvider client={queryClient}>
        <AuthGate>
          <AppContent />
        </AuthGate>
      </QueryClientProvider>
    </AppErrorBoundary>
  );
}
