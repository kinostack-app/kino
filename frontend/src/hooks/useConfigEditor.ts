import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useCallback, useEffect, useState } from 'react';
import { getConfig, updateConfig } from '@/api/generated/sdk.gen';
import type { ConfigUpdate } from '@/api/generated/types.gen';
import { CONFIG_KEY } from '@/state/library-cache';

/**
 * Loose map type for working with config rows as a flat key/value
 * shape. The server accepts a `ConfigUpdate` patch (only the fields
 * that changed); we narrow to that at the save boundary so drift on
 * any individual field would fail the build.
 */
type ConfigData = Record<string, unknown>;

/**
 * Cheap equality for config values. Every column is a primitive, a
 * JSON string (items / accepted_languages), or null, so strict
 * equality is enough. We normalise the common `null` ↔ `''` ↔ `undefined`
 * gap — the server returns null for empty optional strings, the UI
 * passes '' from cleared inputs, and treating those as distinct would
 * show phantom changes after every field touch.
 */
function sameValue(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  const aEmpty = a === null || a === undefined || a === '';
  const bEmpty = b === null || b === undefined || b === '';
  return aEmpty && bEmpty;
}

/**
 * Hook for editing config with change tracking and save/discard.
 */
export function useConfigEditor() {
  const queryClient = useQueryClient();

  const { data: serverConfig, isLoading } = useQuery({
    queryKey: [...CONFIG_KEY],
    queryFn: async () => {
      const { data } = await getConfig();
      return data as ConfigData;
    },
    staleTime: 60_000,
  });

  const [localConfig, setLocalConfig] = useState<ConfigData>({});
  const [changes, setChanges] = useState<ConfigData>({});

  // Sync server config to local state
  useEffect(() => {
    if (serverConfig) {
      setLocalConfig(serverConfig);
      setChanges({});
    }
  }, [serverConfig]);

  const hasChanges = Object.keys(changes).length > 0;

  const updateField = useCallback(
    (key: string, value: unknown) => {
      setLocalConfig((prev) => ({ ...prev, [key]: value }));
      // Only mark the field as dirty when it genuinely differs from
      // what the server currently has. Guards against two issues:
      //   1. "Detect" / preset actions that re-apply the current
      //      value — they used to flash the Save bar on for nothing.
      //   2. User types A, then types back the original — we now
      //      treat that as no change and remove the key from
      //      `changes` instead of accumulating a no-op.
      setChanges((prev) => {
        const next = { ...prev };
        if (serverConfig && sameValue(serverConfig[key], value)) {
          delete next[key];
        } else {
          next[key] = value;
        }
        return next;
      });
    },
    [serverConfig]
  );

  const discard = useCallback(() => {
    if (serverConfig) {
      setLocalConfig(serverConfig);
      setChanges({});
    }
  }, [serverConfig]);

  const saveMutation = useMutation({
    mutationFn: async () => {
      // Only send changed fields
      // Narrow the dynamic field-edit map to the generated
      // `ConfigUpdate` shape at the save boundary so a renamed or
      // removed backend field would surface as a build error at
      // codegen time, not a silent 400.
      await updateConfig({ body: changes as ConfigUpdate });
    },
    onSuccess: () => {
      setChanges({});
      queryClient.invalidateQueries({ queryKey: [...CONFIG_KEY] });
    },
  });

  return {
    config: localConfig,
    changes,
    isLoading,
    hasChanges,
    updateField,
    discard,
    save: () => saveMutation.mutate(),
    isSaving: saveMutation.isPending,
    saveError: saveMutation.error,
  };
}
