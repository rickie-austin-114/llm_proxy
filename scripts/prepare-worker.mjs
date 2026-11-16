import { copyFileSync, mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';

export function prepareWorker(repoRoot, env, args = []) {
  const compiler = spawnSync('rustc', ['-vV'], { env, encoding: 'utf8' });
  if (compiler.error || compiler.status !== 0) throw new Error('Cannot inspect the Rust host.');
  const host = compiler.stdout.match(/^host: (.+)$/m)?.[1];
  const targetOption = args.find((arg) => arg.startsWith('--target='));
  const targetIndex = args.findIndex((arg) => arg === '--target' || arg === '-t');
  const target =
    targetOption?.slice('--target='.length) ?? (targetIndex >= 0 ? args[targetIndex + 1] : host);
  if (!target || target === 'universal-apple-darwin') {
    throw new Error(
      'Build Apple Silicon and Intel installers separately using their Rust targets.',
    );
  }
  const cargoArgs = [
    'build',
    '--release',
    '--locked',
    '-p',
    'proxy-service',
    '--bin',
    'llm-proxy-worker',
  ];
  if (target !== host) cargoArgs.push('--target', target);
  const build = spawnSync('cargo', cargoArgs, { cwd: repoRoot, env, stdio: 'inherit' });
  if (build.error || build.status !== 0) throw new Error('The background worker build failed.');
  const suffix = target.includes('windows') ? '.exe' : '';
  const output =
    target === host
      ? join(repoRoot, 'target', 'release')
      : join(repoRoot, 'target', target, 'release');
  const directory = join(repoRoot, 'src-tauri', 'binaries');
  mkdirSync(directory, { recursive: true });
  copyFileSync(
    join(output, `llm-proxy-worker${suffix}`),
    join(directory, `llm-proxy-worker-${target}${suffix}`),
  );
}
