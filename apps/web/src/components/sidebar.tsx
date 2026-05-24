'use client';

import Link from 'next/link';
import { usePathname } from 'next/navigation';

const NAV = [
  { href: '/', label: 'Operations',    icon: '◆', desc: 'live tool stats, latency budget' },
  { href: '/query-lab', label: 'Query Lab',    icon: '☰', desc: 'dense / sparse / RRF / rerank' },
  { href: '/tools', label: 'Tool Explorer',    icon: '⌘', desc: 'schema-driven forms' },
  { href: '/skills', label: 'Skill Lab',    icon: '✱', desc: 'instantiate prompt templates' },
  { href: '/resources', label: 'Resources',    icon: '⊞', desc: 'sap-rfc / sap-table / agents' },
];

export function Sidebar() {
  const pathname = usePathname();
  return (
    <aside className="w-60 border-r border-ink-800 px-4 py-5 bg-ink-900 sticky top-0 h-screen flex flex-col">
      <Link href="/" className="block mb-6 group">
        <div className="text-accent-500 font-bold tracking-wide">SAP-Automate</div>
        <div className="text-xs text-ink-600 mt-0.5">operator console / v0.1</div>
      </Link>
      <nav className="space-y-1 flex-1">
        {NAV.map((item) => {
          const active = pathname === item.href;
          return (
            <Link
              key={item.href}
              href={item.href}
              className={`block rounded px-2.5 py-2 transition ${
                active
                  ? 'bg-ink-700 text-accent-500'
                  : 'hover:bg-ink-800 text-zinc-300'
              }`}
            >
              <div className="flex items-center gap-2 leading-none">
                <span className="opacity-70">{item.icon}</span>
                <span className="font-medium">{item.label}</span>
              </div>
              <div className="text-[11px] text-ink-600 mt-1 leading-tight pl-5">{item.desc}</div>
            </Link>
          );
        })}
      </nav>
      <div className="text-[10px] text-ink-600 leading-relaxed mt-4 border-t border-ink-800 pt-3">
        MCP 2025-06-18<br />
        rust core / next.js shell
      </div>
    </aside>
  );
}
