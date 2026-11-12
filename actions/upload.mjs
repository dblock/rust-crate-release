import { createHash, randomUUID } from 'node:crypto';
import { lstat, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const digest = bytes => createHash('sha256').update(bytes).digest('hex');

export async function uploadRelease(directory, name, version, checksum, { client, env = process.env } = {}) {
  if (env.GITHUB_ACTIONS !== 'true' || !/^\d+$/.test(env.GITHUB_RUN_ID ?? '') ||
      !/^\d+$/.test(env.GITHUB_RUN_ATTEMPT ?? '') ||
      !/^[a-zA-Z0-9_-]+$/.test(name) || !/^\d+\.\d+\.\d+$/.test(version) ||
      !/^[a-f0-9]{64}$/.test(checksum)) {
    throw new Error('Invalid artifact preservation identity');
  }
  const archive = `${name}-${version}.crate`;
  const bundle = `${name}-${version}.bundle`;
  const selected = [archive, bundle, 'release.json', 'execution.json', 'source-inputs.json'];
  const staging = await mkdtemp(join(tmpdir(), 'rcr-preserve-'));
  try {
    const files = [];
    for (const filename of selected) {
      const source = join(directory, filename);
      let metadata;
      try {
        metadata = await lstat(source);
      } catch (error) {
        if (error.code === 'ENOENT' && filename !== archive && filename !== bundle) continue;
        throw error;
      }
      if (!metadata.isFile()) throw new Error('Preservation requires regular files');
      const bytes = await readFile(source);
      if (filename === archive && digest(bytes) !== checksum) throw new Error('Archive changed before preservation');
      if ((filename === archive || filename === bundle) && bytes.length === 0) throw new Error('Empty release artifact');
      const destination = join(staging, filename);
      await writeFile(destination, bytes, { mode: 0o600, flag: 'wx' });
      files.push(destination);
    }
    const artifactClient = client ?? new (await import('@actions/artifact')).DefaultArtifactClient();
    const result = await artifactClient.uploadArtifact(
      `rcr-release-${name}-${version}-${env.GITHUB_RUN_ID}-${env.GITHUB_RUN_ATTEMPT}-${randomUUID()}`,
      files, staging, { compressionLevel: 0 },
    );
    if (!Number.isSafeInteger(result.id) || result.id <= 0 || !Number.isSafeInteger(result.size) || result.size <= 0) {
      throw new Error('Artifact service did not confirm a durable upload');
    }
    if (digest(await readFile(join(staging, archive))) !== checksum) throw new Error('Preserved archive changed');
    return { id: result.id, size: result.size, files: files.map(file => basename(file)) };
  } finally {
    await rm(staging, { recursive: true, force: true });
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    if (process.argv.length !== 6) throw new Error('Invalid uploader arguments');
    await uploadRelease(...process.argv.slice(2));
  } catch {
    console.error('Durable artifact preservation failed.');
    process.exitCode = 1;
  }
}
