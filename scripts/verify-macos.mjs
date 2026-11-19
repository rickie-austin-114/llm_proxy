import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

function run(command, args, { includeStderr = false, ...options } = {}) {
  const result = spawnSync(command, args, { encoding: 'utf8', ...options });
  if (result.error || result.status !== 0) {
    throw new Error(
      `${command} failed: ${result.error?.message ?? ([result.stderr?.trim(), result.stdout?.trim()].filter(Boolean).join('\n') || `exit ${result.status}`)}`,
    );
  }
  return `${result.stdout ?? ''}${includeStderr ? (result.stderr ?? '') : ''}`;
}

function signature(path) {
  const details = run('/usr/bin/codesign', ['--display', '--verbose=4', path], {
    includeStderr: true,
  });
  if (/flags=.*\blinker-signed\b/.test(details)) {
    throw new Error(`${path}: incomplete linker signature; rebuild the complete signed app.`);
  }
  const team = details.match(/^TeamIdentifier=(.+)$/m)?.[1];
  const cdhash = details.match(/^CDHash=(.+)$/m)?.[1];
  if (!team || !cdhash) throw new Error(`${path}: missing code-signature metadata.`);
  return { team, cdhash, developerId: /^Authority=Developer ID Application:/m.test(details) };
}

export function verifyMacOSApp(appPath, { requireDeveloperId = false } = {}) {
  const workerPath = join(appPath, 'Contents', 'MacOS', 'llm-proxy-worker');
  const agentPath = join(
    appPath,
    'Contents',
    'Library',
    'LaunchAgents',
    'app.llmproxy.worker.plist',
  );
  const sealPath = join(appPath, 'Contents', '_CodeSignature', 'CodeResources');
  if (!existsSync(sealPath)) {
    throw new Error(`${appPath}: missing bundle resource seal; rebuild the complete signed app.`);
  }
  run('/usr/bin/codesign', ['--verify', '--deep', '--strict', appPath]);
  run('/usr/bin/codesign', ['--verify', '--strict', workerPath]);
  const bundleProgram = run('/usr/bin/plutil', [
    '-extract',
    'BundleProgram',
    'raw',
    '-o',
    '-',
    agentPath,
  ]).trim();
  if (bundleProgram !== 'Contents/MacOS/llm-proxy-worker') {
    throw new Error(`${appPath}: bundled LaunchAgent does not point to the worker.`);
  }
  const app = signature(appPath);
  const worker = signature(workerPath);
  if (app.team !== worker.team) {
    throw new Error(`${appPath}: app and worker have different signing teams.`);
  }
  if (requireDeveloperId && (!app.developerId || !worker.developerId || app.team === 'not set')) {
    throw new Error(
      `${appPath}: a Developer ID Application signature is required for distribution.`,
    );
  }
  return { app, worker };
}

function verifyDiskImage(dmgPath, appName, expected, options) {
  run('/usr/bin/hdiutil', ['verify', dmgPath]);
  const directory = mkdtempSync(join(tmpdir(), 'ica-proxy-verify-'));
  const mount = join(directory, 'image');
  let attached = false;
  try {
    run('/usr/bin/hdiutil', [
      'attach',
      '-readonly',
      '-nobrowse',
      '-noautoopen',
      '-mountpoint',
      mount,
      dmgPath,
    ]);
    attached = true;
    const actual = verifyMacOSApp(join(mount, appName), options);
    if (
      actual.app.cdhash !== expected.app.cdhash ||
      actual.worker.cdhash !== expected.worker.cdhash
    ) {
      throw new Error(`${dmgPath}: installer app differs from the verified build.`);
    }
  } finally {
    // Never recursively remove a mount point, including when detach fails.
    if (attached) run('/usr/bin/hdiutil', ['detach', mount, '-quiet']);
    if (existsSync(mount)) rmdirSync(mount);
    rmdirSync(directory);
  }
}

export function verifyMacOSArtifacts(bundleDirectory, options = {}) {
  if (process.platform !== 'darwin') throw new Error('macOS package verification requires macOS.');
  const config = JSON.parse(
    readFileSync(new URL('../src-tauri/tauri.conf.json', import.meta.url), 'utf8'),
  );
  const appName = `${config.productName}.app`;
  const expected = verifyMacOSApp(join(bundleDirectory, 'macos', appName), options);
  const dmgDirectory = join(bundleDirectory, 'dmg');
  const images = existsSync(dmgDirectory)
    ? readdirSync(dmgDirectory).filter((name) => name.endsWith('.dmg'))
    : [];
  if (options.requireDmg && images.length === 0) throw new Error('The macOS DMG is missing.');
  for (const image of images)
    verifyDiskImage(join(dmgDirectory, image), appName, expected, options);
  console.log(
    `Verified ${appName}, its worker, and ${images.length} DMG(s). ` +
      (expected.app.developerId
        ? 'Developer ID signing verified; native startup and notarization acceptance are separate checks.'
        : 'Development ad-hoc signing only; native background registration still requires testing.'),
  );
}

export function verifyMacOSBuild(repoRoot, env, args) {
  const options = args.slice(0, args.indexOf('--') < 0 ? args.length : args.indexOf('--'));
  if (
    process.platform !== 'darwin' ||
    !['build', 'bundle'].includes(options[0]) ||
    options.some((arg) => ['--no-bundle', '--help', '-h', '--version', '-V'].includes(arg))
  )
    return;
  const targetIndex = options.findIndex((arg) => arg === '--target' || arg === '-t');
  const target =
    options.find((arg) => arg.startsWith('--target='))?.slice('--target='.length) ??
    (targetIndex >= 0 ? options[targetIndex + 1] : env.CARGO_BUILD_TARGET);
  if (target && !target.endsWith('apple-darwin')) return;
  const metadata = JSON.parse(
    run('cargo', ['metadata', '--no-deps', '--format-version=1'], {
      cwd: repoRoot,
      env,
    }),
  );
  const profile = options.includes('--debug') || options.includes('-d') ? 'debug' : 'release';
  const bundleDirectory = join(
    metadata.target_directory,
    ...(target ? [target] : []),
    profile,
    'bundle',
  );
  verifyMacOSArtifacts(bundleDirectory);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const args = process.argv.slice(2);
    const directory = args.find((arg) => !arg.startsWith('--')) ?? 'target/release/bundle';
    if (
      args.some(
        (arg) => arg.startsWith('--') && !['--require-dmg', '--require-developer-id'].includes(arg),
      )
    ) {
      throw new Error(
        'Usage: node scripts/verify-macos.mjs [bundle-directory] [--require-dmg] [--require-developer-id]',
      );
    }
    verifyMacOSArtifacts(resolve(directory), {
      requireDmg: args.includes('--require-dmg'),
      requireDeveloperId: args.includes('--require-developer-id'),
    });
  } catch (error) {
    console.error(`macOS package verification failed: ${error.message}`);
    process.exitCode = 1;
  }
}
