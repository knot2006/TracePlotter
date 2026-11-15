import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, readFile, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import test from 'node:test';
import { fetchUpstream, updateSkills } from './update-beautifulrust.mjs';

const revision = 'b'.repeat(40);
const originalRevision = 'a'.repeat(40);
const skillNames = ['rust-best-practices', 'rust-review', 'rust-port'];

function fixture() {
  const files = new Map([['LICENSE', Buffer.from('MIT License\n')]]);
  for (const skill of skillNames) {
    files.set(`skills/${skill}/SKILL.md`, Buffer.from(`---\nname: ${skill}\ndescription: Rust guidance\n---\n`));
  }
  files.set('skills/rust-best-practices/references/ownership.md', Buffer.from('Borrow deliberately.\n'));
  const tree = [...files].map(([path, content]) => ({
    path, type: 'blob', mode: '100644',
    sha: createHash('sha1').update(`blob ${content.length}\0`).update(content).digest('hex'),
  }));
  const urls = [];
  const request = async (url) => {
    urls.push(url);
    if (url.includes('/commits/')) return { sha: revision };
    if (url.includes('/git/trees/')) return { tree, truncated: false };
    const prefix = `https://raw.githubusercontent.com/morganlinton/BeautifulRust/${revision}/`;
    assert.ok(url.startsWith(prefix), 'downloads must use the resolved commit');
    const content = files.get(url.slice(prefix.length));
    assert.ok(content, `unexpected download: ${url}`);
    return content;
  };
  return { files, tree, urls, request };
}

async function workspace(t) {
  const root = await mkdtemp(join(tmpdir(), 'traceplotter-beautifulrust-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const directory = join(root, '.agents', 'skills');
  await mkdir(directory, { recursive: true });
  await writeFile(join(directory, 'README.md'), `Source: \`${originalRevision}\` (retrieved 2026-11-05).\nLocal instructions.\n`);
  await writeFile(join(root, 'AGENTS.md'), 'Project conventions.\n');
  return { root, directory };
}

test('imports one commit, excludes evaluations, and preserves project instructions', async (t) => {
  const source = fixture();
  source.tree.push({ path: 'skills/rust-port/evals/run.sh', type: 'blob', mode: '100755' });
  source.tree.push({ path: 'mcp/index.ts', type: 'blob', mode: '100644' });
  const { root, directory } = await workspace(t);
  const upstream = await fetchUpstream('v1.0', source.request);
  assert.ok(source.urls[0].endsWith('/commits/v1.0'));
  assert.equal(await updateSkills(root, upstream, '2026-11-15'), true);
  for (const [path, content] of upstream.files) assert.deepEqual(await readFile(join(directory, path)), content);
  assert.equal(await readFile(join(root, 'AGENTS.md'), 'utf8'), 'Project conventions.\n');
  assert.equal(await readFile(join(directory, 'README.md'), 'utf8'),
    `Source: \`${revision}\` (retrieved 2026-11-15).\nLocal instructions.\n`);
  assert.ok(source.urls.every((url) => !url.includes('evals') && !url.includes('mcp/')));
});

test('repeated runs and unrelated upstream commits do not change the source record', async (t) => {
  const { root, directory } = await workspace(t);
  const upstream = await fetchUpstream('main', fixture().request);
  await updateSkills(root, upstream, '2026-11-15');
  for (const [path, content] of upstream.files) {
    await writeFile(join(directory, path), content.toString('utf8').replaceAll('\n', '\r\n'));
  }
  const readme = await readFile(join(directory, 'README.md'), 'utf8');
  assert.equal(await updateSkills(root, { ...upstream, sha: 'c'.repeat(40) }, '2026-11-22'), false);
  assert.equal(await readFile(join(directory, 'README.md'), 'utf8'), readme);
});

test('removes stale vendored references and keeps unrelated local files', async (t) => {
  const { root, directory } = await workspace(t);
  const stale = join(directory, 'rust-best-practices', 'references', 'old.md');
  await mkdir(dirname(stale), { recursive: true });
  await writeFile(stale, 'Obsolete guidance.');
  await writeFile(join(directory, 'local-notes.md'), 'Local notes.');
  await updateSkills(root, await fetchUpstream('main', fixture().request));
  await assert.rejects(readFile(stale), { code: 'ENOENT' });
  assert.equal(await readFile(join(directory, 'local-notes.md'), 'utf8'), 'Local notes.');
});

test('rejects incomplete trees, invalid revisions and missing required files', async () => {
  await assert.rejects(fetchUpstream('main', async () => ({ sha: '../main' })), /Invalid upstream commit/);
  await assert.rejects(fetchUpstream('main', async (url) =>
    url.includes('/commits/') ? { sha: revision } : { tree: [], truncated: true }), /Incomplete upstream tree/);
  const source = fixture();
  source.tree.splice(source.tree.findIndex((entry) => entry.path === 'skills/rust-port/SKILL.md'), 1);
  await assert.rejects(fetchUpstream('main', source.request), /Missing or invalid skill: rust-port/);
  const unlicensed = fixture();
  unlicensed.tree.splice(0, 1);
  await assert.rejects(fetchUpstream('main', unlicensed.request), /Missing upstream license/);
});

test('refuses path traversal, symlinks and unsupported upstream layouts', async () => {
  for (const entry of [
    { path: 'skills/rust-port/../../AGENTS.md', mode: '100644' },
    { path: 'skills/rust-port/references/link.md', mode: '120000' },
    { path: 'skills/rust-port/scripts/run.sh', mode: '100755' },
  ]) {
    const source = fixture();
    source.tree.push({ ...entry, type: 'blob' });
    await assert.rejects(fetchUpstream('main', source.request), /Review new upstream file layout/);
  }
});

test('a corrupt or interrupted download leaves installed guidance untouched', async (t) => {
  const { root, directory } = await workspace(t);
  const before = await readFile(join(directory, 'README.md'));
  for (const interrupted of [false, true]) {
    const source = fixture();
    const request = async (url, json) => {
      if (url.startsWith('https://raw.githubusercontent.com/')) {
        if (interrupted) throw new Error('Connection lost');
        return Buffer.from('corrupted');
      }
      return source.request(url, json);
    };
    await assert.rejects(async () => updateSkills(root, await fetchUpstream('main', request)),
      interrupted ? /Connection lost/ : /hash mismatch/);
  }
  assert.deepEqual(await readFile(join(directory, 'README.md')), before);
  await assert.rejects(readFile(join(directory, 'LICENSE-BeautifulRust')), { code: 'ENOENT' });
});

test('refuses an update without provenance before writing any files', async (t) => {
  const { root, directory } = await workspace(t);
  await writeFile(join(directory, 'README.md'), 'Missing revision.');
  await assert.rejects(updateSkills(root, await fetchUpstream('main', fixture().request)), /Missing source revision/);
  await assert.rejects(readFile(join(directory, 'LICENSE-BeautifulRust')), { code: 'ENOENT' });
});

test('refuses local directory links before writing outside the skills directory', async (t) => {
  const { root, directory } = await workspace(t);
  const external = join(root, 'external');
  await mkdir(external);
  await symlink(external, join(directory, 'rust-port'), process.platform === 'win32' ? 'junction' : 'dir');
  await assert.rejects(updateSkills(root, await fetchUpstream('main', fixture().request)), /Refusing local symlink/);
  await assert.rejects(readFile(join(external, 'SKILL.md')), { code: 'ENOENT' });
});
