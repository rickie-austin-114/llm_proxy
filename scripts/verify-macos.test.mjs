import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, beforeEach, describe, it } from 'node:test';
import { verifyMacOSApp, verifyMacOSArtifacts } from './verify-macos.mjs';

describe('macOS package signatures', { skip: process.platform !== 'darwin' }, () => {
  let directory;
  let app;
  let contents;

  function sign(path) {
    const result = spawnSync('/usr/bin/codesign', ['--force', '--sign', '-', path], {
      encoding: 'utf8',
    });
    assert.equal(result.status, 0, result.stderr);
  }

  beforeEach(() => {
    directory = mkdtempSync(join(tmpdir(), 'ica-signature-test-'));
    app = join(directory, 'macos', 'ICA proxy.app');
    contents = join(app, 'Contents');
    const executables = join(contents, 'MacOS');
    const agents = join(contents, 'Library', 'LaunchAgents');
    mkdirSync(executables, { recursive: true });
    mkdirSync(agents, { recursive: true });
    mkdirSync(join(contents, 'Resources'));
    // Disposable, never-executed native binaries; no developer certificate or Keychain access.
    for (const name of ['llm-proxy', 'llm-proxy-worker']) {
      copyFileSync('/usr/bin/true', join(executables, name));
    }
    writeFileSync(
      join(contents, 'Info.plist'),
      `<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>llm-proxy</string>
<key>CFBundleIdentifier</key><string>app.llmproxy.desktop</string>
<key>CFBundlePackageType</key><string>APPL</string>
</dict></plist>`,
    );
    copyFileSync(
      new URL('../src-tauri/macos/app.llmproxy.worker.plist', import.meta.url),
      join(agents, 'app.llmproxy.worker.plist'),
    );
    writeFileSync(join(contents, 'Resources', 'LICENSE'), 'Disposable test resource.');
    sign(join(executables, 'llm-proxy-worker'));
    sign(app);
  });

  afterEach(() => rmSync(directory, { recursive: true, force: true }));

  it('accepts a complete ad-hoc signed app and independently signed worker', () => {
    const result = verifyMacOSApp(app);
    assert.equal(result.app.team, 'not set');
    assert.equal(result.worker.team, 'not set');
  });

  it('rejects the missing resource seal found in the installed app', () => {
    rmSync(join(contents, '_CodeSignature'), { recursive: true });
    assert.throws(() => verifyMacOSApp(app), /missing bundle resource seal/);
  });

  it('rejects a resource modified after signing', () => {
    writeFileSync(join(contents, 'Resources', 'LICENSE'), 'Modified after signing.');
    assert.throws(() => verifyMacOSApp(app), /codesign failed/);
  });

  it('rejects a worker replaced after app signing', () => {
    writeFileSync(join(contents, 'MacOS', 'llm-proxy-worker'), 'Invalid replacement.');
    assert.throws(() => verifyMacOSApp(app), /codesign failed/);
  });

  it('rejects a correctly signed package with a broken LaunchAgent target', () => {
    writeFileSync(
      join(contents, 'Library', 'LaunchAgents', 'app.llmproxy.worker.plist'),
      '<plist version="1.0"><dict><key>BundleProgram</key><string>missing</string></dict></plist>',
    );
    sign(app);
    assert.throws(() => verifyMacOSApp(app), /LaunchAgent does not point to the worker/);
  });

  it('rejects ad-hoc signing when Developer ID distribution is requested', () => {
    assert.throws(
      () => verifyMacOSApp(app, { requireDeveloperId: true }),
      /Developer ID Application/,
    );
  });

  it('rejects a missing installer when a DMG is required', () => {
    assert.throws(() => verifyMacOSArtifacts(directory, { requireDmg: true }), /DMG is missing/);
  });

  it('checks the current product when an old product bundle is also present', () => {
    mkdirSync(join(directory, 'macos', 'llm_proxy.app'));
    assert.doesNotThrow(() => verifyMacOSArtifacts(directory));
  });
});
