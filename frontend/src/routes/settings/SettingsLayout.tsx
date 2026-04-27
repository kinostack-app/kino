import { Link, Outlet, useRouterState } from '@tanstack/react-router';
import {
  Archive,
  Bell,
  Clock,
  Download,
  Film,
  FolderOpen,
  Globe,
  HardDrive,
  Link2,
  type LucideIcon,
  Play,
  ScrollText,
  Server,
  Shield,
  Sliders,
  Smartphone,
} from 'lucide-react';
import { createContext, useContext } from 'react';
import { SaveBar } from '@/components/settings/SaveBar';
import { useConfigEditor } from '@/hooks/useConfigEditor';
import { useDocumentTitle } from '@/hooks/useDocumentTitle';
import { cn } from '@/lib/utils';

export type SettingsCategory =
  | 'general'
  | 'library'
  | 'metadata'
  | 'vpn'
  | 'indexers'
  | 'quality'
  | 'downloads'
  | 'playback'
  | 'automation'
  | 'notifications'
  | 'integrations'
  | 'devices'
  | 'tasks'
  | 'backup'
  | 'logs';

interface NavItem {
  id: SettingsCategory;
  label: string;
  icon: LucideIcon;
}

const navItems: NavItem[] = [
  { id: 'general', label: 'General', icon: Server },
  { id: 'library', label: 'Media Library', icon: FolderOpen },
  { id: 'metadata', label: 'Metadata', icon: Globe },
  { id: 'vpn', label: 'VPN', icon: Shield },
  { id: 'indexers', label: 'Indexers', icon: HardDrive },
  { id: 'quality', label: 'Quality Profiles', icon: Sliders },
  { id: 'downloads', label: 'Downloads', icon: Download },
  { id: 'playback', label: 'Playback', icon: Play },
  { id: 'automation', label: 'Automation', icon: Film },
  { id: 'notifications', label: 'Notifications', icon: Bell },
  { id: 'integrations', label: 'Integrations', icon: Link2 },
  { id: 'devices', label: 'Devices', icon: Smartphone },
  { id: 'tasks', label: 'Tasks', icon: Clock },
  { id: 'backup', label: 'Backup', icon: Archive },
  { id: 'logs', label: 'Logs', icon: ScrollText },
];

// Shared editor context — scalar settings pages read `config` + `updateField`
// from here instead of receiving them as props, so nested routes can be
// independent while still sharing one pending-changes buffer + save bar.
type Editor = ReturnType<typeof useConfigEditor>;
const SettingsContext = createContext<Editor | null>(null);

export function useSettingsContext(): Editor {
  const ctx = useContext(SettingsContext);
  if (!ctx) {
    throw new Error('useSettingsContext must be used inside SettingsLayout');
  }
  return ctx;
}

export function SettingsLayout() {
  const router = useRouterState();
  const editor = useConfigEditor();

  // Active = second segment of the current path ('general', 'indexers', …).
  const active = router.location.pathname.split('/')[2] ?? 'general';

  // Derive the document title from the sidebar label for this category.
  // Single source of truth — adding a new settings route updates the
  // title automatically via the navItems list below.
  const activeLabel = navItems.find((n) => n.id === active)?.label ?? 'Settings';
  useDocumentTitle(`Settings · ${activeLabel}`);

  return (
    <SettingsContext.Provider value={editor}>
      <div className="flex min-h-[calc(100vh-3.5rem)] pb-24 md:pb-0">
        {/* Sidebar — hidden on mobile, shown as horizontal scroll */}
        <nav className="hidden md:block w-52 lg:w-56 flex-shrink-0 border-r border-white/5 py-4 px-2">
          <h2 className="px-3 mb-3 text-xs font-semibold text-[var(--text-muted)] uppercase tracking-wider">
            Settings
          </h2>
          <div className="space-y-0.5">
            {navItems.map((item) => (
              <Link
                key={item.id}
                to={`/settings/${item.id}`}
                className={cn(
                  'w-full flex items-center gap-2.5 px-3 py-2 rounded-lg text-sm transition-colors text-left',
                  active === item.id
                    ? 'bg-white/10 text-white font-medium'
                    : 'text-[var(--text-secondary)] hover:text-white hover:bg-white/5'
                )}
              >
                <item.icon size={16} className="flex-shrink-0" />
                {item.label}
              </Link>
            ))}
          </div>
        </nav>

        {/* Mobile nav — horizontal scroll */}
        <div className="md:hidden fixed top-14 left-0 right-0 z-30 bg-[var(--bg-primary)] border-b border-white/5">
          <div className="flex overflow-x-auto scrollbar-hide px-4 py-2 gap-1">
            {navItems.map((item) => (
              <Link
                key={item.id}
                to={`/settings/${item.id}`}
                className={cn(
                  'flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium whitespace-nowrap flex-shrink-0 transition-colors',
                  active === item.id
                    ? 'bg-white/10 text-white'
                    : 'text-[var(--text-muted)] hover:text-white'
                )}
              >
                <item.icon size={12} />
                {item.label}
              </Link>
            ))}
          </div>
        </div>

        {/* Content — scalar settings pages stay readable at max-w-3xl; the
            Logs page is a dense table and uses the full width. */}
        <div
          className={cn(
            'flex-1 min-w-0 px-4 md:px-8 py-6 mt-12 md:mt-0',
            active === 'logs' ? 'max-w-none' : 'max-w-3xl'
          )}
        >
          <Outlet />
          <SaveBar
            hasChanges={editor.hasChanges}
            changes={editor.changes}
            onSave={editor.save}
            onDiscard={editor.discard}
            isSaving={editor.isSaving}
            saveError={editor.saveError}
          />
        </div>
      </div>
    </SettingsContext.Provider>
  );
}
