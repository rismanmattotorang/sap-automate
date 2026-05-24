import type { Config } from 'tailwindcss';

const config: Config = {
  content: ['./src/**/*.{ts,tsx}'],
  theme: {
    extend: {
      colors: {
        ink: { 950: '#08090b', 900: '#0c0e12', 800: '#13161c', 700: '#1f242e', 600: '#2c3340' },
        accent: { 500: '#22d3ee', 600: '#0891b2' },
        good: '#22c55e',
        warn: '#eab308',
        bad: '#ef4444',
      },
      fontFamily: {
        mono: ['ui-monospace', 'SFMono-Regular', 'Menlo', 'monospace'],
      },
    },
  },
  plugins: [],
};
export default config;
