import * as esbuild from 'esbuild';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

const prod = process.env.NODE_ENV === 'production' || process.argv.includes('--prod');
const watch = process.argv.includes('--watch');

const outdir = process.env.DIST_DIR
  ? path.resolve(process.env.DIST_DIR)
  : path.resolve(__dirname, '../themes/build/static/dist');

const opts = {
  entryPoints: [
    { out: 'js/dashboard', in: path.resolve(__dirname, 'src/dashboard/main.js') },
    { out: 'css/dashboard', in: path.resolve(__dirname, 'src/dashboard/styles/index.css') },
  ],
  bundle: true,
  format: 'iife',
  target: ['es2022'],
  outdir,
  assetNames: 'assets/[name]-[hash]',
  loader: { '.css': 'css' },
  minify: prod,
  sourcemap: prod ? false : 'linked',
  logLevel: 'info',
  metafile: true,
};

if (watch) {
  const ctx = await esbuild.context(opts);
  await ctx.watch();
  console.log(`[esbuild] watching, output: ${outdir}`);
} else {
  await esbuild.build(opts);
}
