import { existsSync, readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { delimiter, join } from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { prepareWorker } from './prepare-worker.mjs';

const repoRoot = fileURLToPath(new URL('../', import.meta.url));
const require = createRequire(import.meta.url);

function rustEnvironment() {
  const rustRoot = join(repoRoot, '.toolchains', 'rust');
  if (!existsSync(rustRoot)) return process.env;

  const host = {
    'darwin-arm64': 'aarch64-apple-darwin',
    'darwin-x64': 'x86_64-apple-darwin',
    'win32-x64': 'x86_64-pc-windows-msvc',
    'win32-arm64': 'aarch64-pc-windows-msvc',
  }[`${process.platform}-${process.arch}`];
  const executableSuffix = process.platform === 'win32' ? '.exe' : '';
  const cargoHome = join(rustRoot, 'cargo');
  const rustupHome = join(rustRoot, 'rustup');
  const bin = join(cargoHome, 'bin');
  if (!host || !existsSync(join(bin, `rustup${executableSuffix}`))) {
    throw new Error(
      'The .toolchains/rust bundle does not support this host or is incomplete. ' +
        'Use a bundle for your OS and processor, or move it aside to use system Rust.',
    );
  }

  const pin = readFileSync(join(repoRoot, 'rust-toolchain.toml'), 'utf8').match(
    /^channel\s*=\s*"([^"]+)"\s*$/m,
  )?.[1];
  if (!pin) throw new Error('Cannot read the pinned channel from rust-toolchain.toml.');

  // The copied installation is named stable; verify its release against the pin
  // before selecting it. An absolute toolchain path survives moving the repo.
  const toolchain = [pin, 'stable']
    .map((channel) => join(rustupHome, 'toolchains', `${channel}-${host}`))
    .find((directory) => existsSync(join(directory, 'bin', `rustc${executableSuffix}`)));
  if (!toolchain) {
    throw new Error(`The local Rust bundle has no toolchain for ${host} (requires ${pin}).`);
  }

  const env = { ...process.env };
  // Windows environment keys are case insensitive. Avoid leaving duplicate PATH
  // spellings when handing the environment to a child process.
  const pathKey = Object.keys(env).find((key) => key.toUpperCase() === 'PATH');
  const originalPath = pathKey ? env[pathKey] : '';
  if (pathKey) delete env[pathKey];
  Object.assign(env, {
    PATH: `${bin}${delimiter}${originalPath ?? ''}`,
    CARGO_HOME: cargoHome,
    RUSTUP_HOME: rustupHome,
    RUSTUP_TOOLCHAIN: toolchain,
  });

  const compiler = spawnSync(join(toolchain, 'bin', `rustc${executableSuffix}`), ['-vV'], {
    env,
    encoding: 'utf8',
  });
  if (
    compiler.error ||
    compiler.status !== 0 ||
    !compiler.stdout.split(/\r?\n/).includes(`release: ${pin}`) ||
    !compiler.stdout.split(/\r?\n/).includes(`host: ${host}`)
  ) {
    throw new Error(`The local Rust compiler must be a working ${pin} installation for ${host}.`);
  }
  return env;
}

try {
  const [command, ...args] = process.argv.slice(2);
  if (!['tauri', 'cargo', 'rustc', 'rustup', 'worker'].includes(command)) {
    throw new Error('Usage: node scripts/with-rust.mjs <tauri|cargo|rustc|rustup> [arguments...]');
  }
  const env = rustEnvironment();
  if (command === 'worker' || (command === 'tauri' && ['build', 'dev'].includes(args[0]))) {
    prepareWorker(repoRoot, env, args);
  }
  if (command === 'worker') process.exit(0);
  const child =
    command === 'tauri'
      ? spawn(process.execPath, [require.resolve('@tauri-apps/cli/tauri.js'), ...args], {
          env,
          stdio: 'inherit',
        })
      : spawn(command, args, { env, stdio: 'inherit' });
  child.on('error', (error) => {
    console.error(`Cannot launch ${command}: ${error.message}`);
    process.exitCode = 1;
  });
  child.on('exit', (code, signal) => {
    if (signal) process.kill(process.pid, signal);
    else process.exitCode = code ?? 1;
  });
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
}
