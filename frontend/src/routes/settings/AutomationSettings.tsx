import { DurationInput, FormField, Toggle } from '@/components/settings/FormField';
import { useSettingsContext } from './SettingsLayout';

export function AutomationSettings() {
  const { config, updateField } = useSettingsContext();
  return (
    <div>
      <h1 className="text-xl font-bold mb-1">Automation</h1>
      <p className="text-sm text-[var(--text-muted)] mb-6">
        Search intervals, cleanup, and upgrades
      </p>

      <section className="space-y-1 border-b border-white/5 pb-6 mb-6">
        <h2 className="text-sm font-semibold text-[var(--text-secondary)] uppercase tracking-wider mb-3">
          Search
        </h2>
        <FormField
          label="Search Interval"
          description="How often to search for wanted content"
          help="Lower values find content faster but hit indexers more often. Most indexers are fine with 15 minutes."
        >
          <DurationInput
            value={Number(config.auto_search_interval ?? 15)}
            onChange={(v) => updateField('auto_search_interval', v)}
            unit="minutes"
            min={5}
          />
        </FormField>
        <FormField
          label="Auto Upgrade"
          description="Search for better quality when available"
          help="When enabled, kino continues searching for higher-quality versions of content you already have, up to the cutoff in your quality profile."
        >
          <Toggle
            checked={Boolean(config.auto_upgrade_enabled)}
            onChange={(v) => updateField('auto_upgrade_enabled', v)}
          />
        </FormField>
      </section>

      <section className="space-y-1">
        <h2 className="text-sm font-semibold text-[var(--text-secondary)] uppercase tracking-wider mb-3">
          Cleanup
        </h2>
        <FormField
          label="Auto Cleanup"
          description="Remove watched content after a delay"
          help="Deletes media files after you've watched them. The movie/show stays in your history but the file is removed to free disk space."
        >
          <Toggle
            checked={Boolean(config.auto_cleanup_enabled)}
            onChange={(v) => updateField('auto_cleanup_enabled', v)}
          />
        </FormField>
        {Boolean(config.auto_cleanup_enabled) && (
          <>
            <FormField
              label="Movie Delay"
              description="After watched, wait before cleanup"
              help="Gives you time to rewatch. Set to 0 for immediate cleanup after marking watched."
            >
              <DurationInput
                value={Number(config.auto_cleanup_movie_delay ?? 72)}
                onChange={(v) => updateField('auto_cleanup_movie_delay', v)}
                unit="hours"
                min={0}
              />
            </FormField>
            <FormField
              label="Episode Delay"
              description="After all season episodes watched"
              help="Cleanup happens at season level — all episodes in a season are removed together after the last one is watched."
            >
              <DurationInput
                value={Number(config.auto_cleanup_episode_delay ?? 72)}
                onChange={(v) => updateField('auto_cleanup_episode_delay', v)}
                unit="hours"
                min={0}
              />
            </FormField>
          </>
        )}
      </section>
    </div>
  );
}
