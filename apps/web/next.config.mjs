/** @type {import('next').NextConfig} */
const config = {
  reactStrictMode: true,
  output: 'standalone',
  experimental: {
    serverActions: { allowedOrigins: ['localhost:3000', '127.0.0.1:3000'] },
  },
};
export default config;
