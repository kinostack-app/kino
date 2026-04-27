import { Link, useRouterState } from '@tanstack/react-router';
import { CalendarDays, Home, Library, ListTodo, Search } from 'lucide-react';
import { cn } from '@/lib/utils';

const tabs = [
  { label: 'Home', icon: Home, href: '/' },
  { label: 'Search', icon: Search, href: '/search' },
  { label: 'Library', icon: Library, href: '/library' },
  { label: 'Lists', icon: ListTodo, href: '/lists' },
  { label: 'Calendar', icon: CalendarDays, href: '/calendar' },
];

export function BottomNav() {
  const router = useRouterState();
  const active = router.location.pathname;

  return (
    <nav
      className="fixed bottom-0 left-0 right-0 z-50 md:hidden h-16 flex items-center justify-around bg-[var(--bg-primary)]/95 backdrop-blur-md border-t border-white/5 pb-safe"
      aria-label="Primary navigation"
    >
      {tabs.map((tab) => {
        const isActive = active === tab.href;
        return (
          <Link
            key={tab.href}
            to={tab.href}
            aria-label={tab.label}
            aria-current={isActive ? 'page' : undefined}
            className={cn(
              'flex flex-col items-center gap-1 px-3 py-1 transition-colors',
              isActive ? 'text-white' : 'text-[var(--text-muted)]'
            )}
          >
            <tab.icon size={20} aria-hidden="true" />
            <span className="text-[10px] font-medium">{tab.label}</span>
          </Link>
        );
      })}
    </nav>
  );
}
