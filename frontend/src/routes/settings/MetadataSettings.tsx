import { ExternalLink } from 'lucide-react';
import { testOpensubtitles, testTmdb } from '@/api/generated/sdk.gen';
import { FormField, SecretInput, TestButton, TextInput } from '@/components/settings/FormField';
import { useSettingsContext } from './SettingsLayout';

export function MetadataSettings() {
  const { config, updateField } = useSettingsContext();

  // Tests hit the backend, which reads the *saved* config. The save
  // bar on the settings layout prompts the user to save before testing
  // unsaved changes.
  const runTmdbTest = async (): Promise<boolean> => {
    try {
      const { data } = await testTmdb();
      return Boolean(data?.ok);
    } catch {
      return false;
    }
  };

  const runOsTest = async (): Promise<boolean> => {
    try {
      const { data } = await testOpensubtitles();
      return Boolean(data?.ok);
    } catch {
      return false;
    }
  };

  return (
    <div>
      <h1 className="text-xl font-bold mb-1">Metadata</h1>
      <p className="text-sm text-[var(--text-muted)] mb-6">External API keys for content data</p>

      <section className="space-y-1 border-b border-white/5 pb-6 mb-6">
        <h2 className="text-sm font-semibold text-[var(--text-secondary)] uppercase tracking-wider mb-3">
          TMDB
        </h2>
        <p className="text-xs text-[var(--text-muted)] mb-3">
          This product uses the TMDB API but is not endorsed or certified by TMDB.
        </p>
        <FormField
          label="API Key"
          description="Required for browse, search, and metadata"
          help="Get a free API Read Access Token from themoviedb.org. This is the long JWT token, not the short API key."
        >
          <SecretInput
            value={String(config.tmdb_api_key ?? '')}
            onChange={(v) => updateField('tmdb_api_key', v)}
            placeholder="Enter TMDB API read access token"
            masked
          />
        </FormField>
        <div className="flex items-center gap-3 mt-2 ml-0 sm:ml-48">
          <TestButton onTest={runTmdbTest} label="Test Connection" />
          <a
            href="https://www.themoviedb.org/settings/api"
            target="_blank"
            rel="noopener noreferrer"
            className="flex items-center gap-1 text-xs text-[var(--accent)] hover:underline"
          >
            <ExternalLink size={11} />
            Get API key
          </a>
          <a
            href="https://www.themoviedb.org/signup"
            target="_blank"
            rel="noopener noreferrer"
            className="flex items-center gap-1 text-xs text-[var(--accent)] hover:underline"
          >
            <ExternalLink size={11} />
            Sign up
          </a>
        </div>
      </section>

      <section className="space-y-1">
        <h2 className="text-sm font-semibold text-[var(--text-secondary)] uppercase tracking-wider mb-3">
          OpenSubtitles
        </h2>
        <p className="text-xs text-[var(--text-muted)] mb-3">
          Optional. Used to auto-download subtitles after import. Requires a free account from
          opensubtitles.com — the API key is separate and must be generated from the account
          consumers page.
        </p>
        <FormField
          label="API Key"
          description="From Consumers page"
          help="Register a free consumer on opensubtitles.com (Profile → Consumers → New consumer) and copy the API key it issues."
        >
          <SecretInput
            value={String(config.opensubtitles_api_key ?? '')}
            onChange={(v) => updateField('opensubtitles_api_key', v)}
            placeholder="Enter API key"
            masked
          />
        </FormField>
        <FormField
          label="Username"
          description="Your opensubtitles.com account"
          help="Same username you use to log in to opensubtitles.com — not your email."
        >
          <TextInput
            value={String(config.opensubtitles_username ?? '')}
            onChange={(v) => updateField('opensubtitles_username', v)}
          />
        </FormField>
        <FormField label="Password" description="Your account password">
          <SecretInput
            value={String(config.opensubtitles_password ?? '')}
            onChange={(v) => updateField('opensubtitles_password', v)}
            masked
          />
        </FormField>
        <div className="flex items-center gap-3 mt-2 ml-0 sm:ml-48">
          <TestButton onTest={runOsTest} label="Test Connection" />
          <a
            href="https://www.opensubtitles.com/en/consumers"
            target="_blank"
            rel="noopener noreferrer"
            className="flex items-center gap-1 text-xs text-[var(--accent)] hover:underline"
          >
            <ExternalLink size={11} />
            Get API key
          </a>
          <a
            href="https://www.opensubtitles.com/en/users/sign_up"
            target="_blank"
            rel="noopener noreferrer"
            className="flex items-center gap-1 text-xs text-[var(--accent)] hover:underline"
          >
            <ExternalLink size={11} />
            Sign up
          </a>
        </div>
      </section>
    </div>
  );
}
