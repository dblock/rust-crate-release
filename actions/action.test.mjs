import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { access, mkdir, mkdtemp, readFile, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, join } from 'node:path';
import test from 'node:test';
import { guard } from './runner.mjs';
import { uploadRelease } from './upload.mjs';

const env = { GITHUB_ACTIONS: 'true', GITHUB_RUN_ID: '42', GITHUB_RUN_ATTEMPT: '1' };
const bytes = Buffer.from('retained exact crate bytes');
const checksum = createHash('sha256').update(bytes).digest('hex');

async function fixture(run) {
  const root = await mkdtemp(join(tmpdir(), 'rcr-action-test-'));
  try {
    const directory = join(root, '.hidden', 'rcr');
    await mkdir(directory, { recursive: true });
    await writeFile(join(directory, 'example-0.1.0.crate'), bytes);
    await writeFile(join(directory, 'example-0.1.0.bundle'), 'Git recovery bundle');
    await writeFile(join(directory, 'release.json'), '{}');
    await writeFile(join(directory, '.env'), 'must never be uploaded');
    await run(directory);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
}

test('uploads a frozen explicit snapshot and awaits durable service confirmation', async () => {
  await fixture(async directory => {
    let staging;
    let finished = false;
    const client = {
      async uploadArtifact(name, files, root, options) {
        staging = root;
        assert.match(name, /^rcr-release-example-0\.1\.0-42-1-/);
        assert.deepEqual(files.map(file => basename(file)), ['example-0.1.0.crate', 'example-0.1.0.bundle', 'release.json']);
        assert.equal(options.compressionLevel, 0);
        assert.notEqual(root, directory);
        await writeFile(join(directory, 'example-0.1.0.crate'), 'concurrent source edit');
        assert.deepEqual(await readFile(files[0]), bytes);
        await new Promise(resolve => setTimeout(resolve, 5));
        finished = true;
        return { id: 123, size: 500 };
      },
    };
    const uploaded = await uploadRelease(directory, 'example', '0.1.0', checksum, { client, env });
    assert.equal(finished, true);
    assert.equal(uploaded.id, 123);
    await assert.rejects(access(staging), { code: 'ENOENT' });
  });
});

test('checksum conflicts, missing bundles, and non-GitHub execution fail before the client', async () => {
  await fixture(async directory => {
    let calls = 0;
    const client = { async uploadArtifact() { calls += 1; return { id: 1, size: 1 }; } };
    await assert.rejects(uploadRelease(directory, 'example', '0.1.0', '0'.repeat(64), { client, env }), /Archive changed/);
    await assert.rejects(uploadRelease(directory, 'example', '0.1.0', checksum, { client, env: {} }), /identity/);
    await rm(join(directory, 'example-0.1.0.bundle'));
    await assert.rejects(uploadRelease(directory, 'example', '0.1.0', checksum, { client, env }), { code: 'ENOENT' });
    assert.equal(calls, 0);
  });
});

test('service rejection and incomplete confirmation cannot report preservation success', async () => {
  await fixture(async directory => {
    for (const result of [{}, { id: 1, size: 0 }]) {
      const client = { async uploadArtifact() { return result; } };
      await assert.rejects(uploadRelease(directory, 'example', '0.1.0', checksum, { client, env }), /did not confirm/);
    }
    const client = { async uploadArtifact() { throw new Error('simulated unavailable artifact service'); } };
    await assert.rejects(uploadRelease(directory, 'example', '0.1.0', checksum, { client, env }), /unavailable/);
  });
});

test('rejects symbolic-link artifacts', { skip: process.platform === 'win32' }, async () => {
  await fixture(async directory => {
    const archive = join(directory, 'example-0.1.0.crate');
    await rm(archive);
    await symlink(join(directory, 'release.json'), archive);
    await assert.rejects(uploadRelease(directory, 'example', '0.1.0', checksum, { env }), /regular files/);
  });
});

test('manual default-branch hosted OIDC guard rejects unsafe caller contexts', () => {
  const valid = {
    ...env, GITHUB_SERVER_URL: 'https://github.com', GITHUB_EVENT_NAME: 'workflow_dispatch',
    GITHUB_REF: 'refs/heads/main', RCR_DEFAULT_BRANCH: 'main', RUNNER_ENVIRONMENT: 'github-hosted',
  };
  guard(valid);
  for (const change of [
    { GITHUB_EVENT_NAME: 'push' }, { GITHUB_REF: 'refs/heads/topic' },
    { RUNNER_ENVIRONMENT: 'self-hosted' }, { CARGO_REGISTRY_TOKEN: 'unexpected-stored-token' },
    { GITHUB_SERVER_URL: 'https://example.invalid' },
  ]) {
    assert.throws(() => guard({ ...valid, ...change }), /manual default-branch/);
  }
});
