import './globals.css';
import type { Metadata } from 'next';
import { Sidebar } from '@/components/sidebar';

export const metadata: Metadata = {
  title: 'SAP-Automate Operator Console',
  description: 'Web UI for the SAP-Automate MCP server.',
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en" className="dark">
      <body className="min-h-screen flex font-mono text-[13.5px] bg-ink-950">
        <Sidebar />
        <main className="flex-1 min-w-0 overflow-auto">{children}</main>
      </body>
    </html>
  );
}
