import { appendFileSync, realpathSync } from 'node:fs';
import { resolve, join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';

export function guard(env) {
  if (env.GITHUB_ACTIONS !== 'true' || env.GITHUB_SERVER_URL !== 'https://github.com' ||
      env.GITHUB_EVENT_NAME !== 'workflow_dispatch' || !env.RCR_DEFAULT_BRANCH ||
      env.GITHUB_REF !== `refs/heads/${env.RCR_DEFAULT_BRANCH}` ||
      env.RUNNER_ENVIRONMENT !== 'github-hosted' || env.CARGO_REGISTRY_TOKEN) {
    throw new Error('Releases require manual default-branch dispatch on github.com hosted runners with OIDC');
  }
}

export function runCommand(program, args, cwd, json = false) {
  const result = spawnSync(program, args, { cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'], maxBuffer: 32 * 1024 * 1024 });
  if (result.error || result.status !== 0) throw new Error(`Release command failed: ${program}`);
  return json ? JSON.parse(result.stdout) : result.stdout.trim();
}

function output(env, name, value) {
  const text = String(value);
  if (!env.GITHUB_OUTPUT || /[\r\n]/.test(text)) throw new Error('Invalid Actions output');
  appendFileSync(env.GITHUB_OUTPUT, `${name}=${text}\n`);
}

export function runAction(mode, env = process.env, execute = runCommand) {
  guard(env);
  const root = realpathSync(resolve(env.RCR_ROOT || '.'));
  const metadata = resolve(root, execute('git', ['rev-parse', '--git-path', 'rcr'], root));
  if (mode === 'guard') {
    output(env, 'directory', metadata);
    return;
  }
  if (mode !== 'release' || !env.RCR_BINARY || !env.RCR_HELPER) throw new Error('Invalid action operation');
  const common = ['--root', root, '--artifact-directory', metadata];
  if (env.RCR_CONFIG) common.push('--config', env.RCR_CONFIG);
  const inputs = join(metadata, 'source-inputs.json');
  const prepare = [...common, 'prepare', '--save-inputs', inputs];
  if (env.RCR_VERSION) prepare.push('--release-version', env.RCR_VERSION);
  execute(env.RCR_BINARY, prepare, root, true);
  const completed = execute(env.RCR_BINARY, [
    ...common, '--github-artifact-uploader', env.RCR_HELPER, 'release', '--inputs', inputs,
  ], root, true);
  for (const name of ['name', 'version', 'archive', 'checksum']) output(env, name, completed[name]);
  output(env, 'verification', JSON.stringify(completed.verification));
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    runAction(process.argv[2]);
  } catch {
    console.error('Release action failed; inspect retained diagnostics and the failed phase.');
    process.exitCode = 1;
  }
}
