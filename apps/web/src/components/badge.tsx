export function Badge({ children, tone = 'neutral' }: {
  children: React.ReactNode;
  tone?: 'neutral' | 'good' | 'warn' | 'bad' | 'accent';
}) {
  const cls = {
    neutral: 'bg-ink-700 text-zinc-300',
    good:    'bg-good/15 text-good',
    warn:    'bg-warn/15 text-warn',
    bad:     'bg-bad/15 text-bad',
    accent:  'bg-accent-500/15 text-accent-500',
  }[tone];
  return (
    <span className={`inline-block rounded px-1.5 py-0.5 text-[11px] leading-none ${cls}`}>
      {children}
    </span>
  );
}

export function PageHeader({ title, subtitle, right }: { title: string; subtitle?: string; right?: React.ReactNode }) {
  return (
    <header className="border-b border-ink-800 px-6 py-4 sticky top-0 bg-ink-950/90 backdrop-blur z-10 flex items-center">
      <div className="flex-1">
        <h1 className="text-lg font-semibold text-zinc-100">{title}</h1>
        {subtitle && <p className="text-xs text-ink-600 mt-1">{subtitle}</p>}
      </div>
      {right}
    </header>
  );
}

export function Card({ title, children, right, dense }: {
  title?: string;
  right?: React.ReactNode;
  children: React.ReactNode;
  dense?: boolean;
}) {
  return (
    <section className="rounded-lg border border-ink-800 bg-ink-900">
      {title && (
        <header className="border-b border-ink-800 px-3 py-2 flex items-center">
          <h2 className="text-xs font-semibold uppercase tracking-wide text-zinc-400 flex-1">{title}</h2>
          {right}
        </header>
      )}
      <div className={dense ? '' : 'p-3'}>{children}</div>
    </section>
  );
}
