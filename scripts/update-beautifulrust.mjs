// Refresh only the vendored guidance; project conventions remain reviewed separately.
import { createHash } from 'node:crypto';
import { mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const repository = 'morganlinton/BeautifulRust';
const skills = ['rust-best-practices', 'rust-review', 'rust-port'];
const sourceRecord = /`[a-f0-9]{40}` \(retrieved \d{4}-\d{2}-\d{2}\)\./;
const markdownPath = /^(rust-best-practices|rust-review|rust-port)\/(SKILL\.md|references\/[a-z0-9][a-z0-9.-]*\.md)$/;

function blobHash(content) {
  return createHash('sha1').update(`blob ${content.length}\0`).update(content).digest('hex');
}

async function download(url, json = false) {
  const headers = { 'User-Agent': 'TracePlotter-BeautifulRust-update' };
  if (url.startsWith('https://api.github.com/') && process.env.GH_TOKEN) {
    headers.Authorization = `Bearer ${process.env.GH_TOKEN}`;
  }
  const response = await fetch(url, { headers, signal: AbortSignal.timeout(30_000) });
  if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
  return json ? response.json() : Buffer.from(await response.arrayBuffer());
}

export async function fetchUpstream(ref = 'main', request = download) {
  const commit = await request(`https://api.github.com/repos/${repository}/commits/${encodeURIComponent(ref)}`, true);
  if (!/^[a-f0-9]{40}$/.test(commit.sha)) throw new Error('Invalid upstream commit');
  const tree = await request(`https://api.github.com/repos/${repository}/git/trees/${commit.sha}?recursive=1`, true);
  if (tree.truncated || !Array.isArray(tree.tree)) throw new Error('Incomplete upstream tree');
  const files = new Map();
  for (const entry of tree.tree) {
    const relative = entry.path.replace(/^skills\//, '');
    if (entry.path !== 'LICENSE' && !skills.some((skill) => entry.path.startsWith(`skills/${skill}/`))) continue;
    if (relative.split('/').includes('evals') || entry.type === 'tree') continue;
    if (entry.mode !== '100644' || (entry.path !== 'LICENSE' && !markdownPath.test(relative))) {
      throw new Error(`Review new upstream file layout: ${entry.path}`);
    }
    const content = await request(`https://raw.githubusercontent.com/${repository}/${commit.sha}/${entry.path}`);
    if (blobHash(content) !== entry.sha) throw new Error(`Upstream hash mismatch: ${entry.path}`);
    new TextDecoder('utf-8', { fatal: true }).decode(content);
    files.set(entry.path === 'LICENSE' ? 'LICENSE-BeautifulRust' : relative, content);
  }
  for (const skill of skills) {
    const content = files.get(`${skill}/SKILL.md`)?.toString('utf8');
    if (!content?.startsWith(`---\nname: ${skill}\n`) || !content.includes('\ndescription:')) {
      throw new Error(`Missing or invalid skill: ${skill}`);
    }
  }
  if (!files.has('LICENSE-BeautifulRust')) throw new Error('Missing upstream license');
  return { sha: commit.sha, files };
}

async function localFiles(directory, prefix = '') {
  const files = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isSymbolicLink()) throw new Error(`Refusing local symlink: ${relative}`);
    if (entry.isDirectory()) files.push(...await localFiles(join(directory, entry.name), relative));
    else files.push(relative);
  }
  return files;
}

export async function updateSkills(root, upstream, date = new Date().toISOString().slice(0, 10)) {
  const directory = join(root, '.agents', 'skills');
  const readmePath = join(directory, 'README.md');
  const readme = await readFile(readmePath, 'utf8');
  if (!sourceRecord.test(readme)) throw new Error('Missing source revision in skills README');
  const current = (await localFiles(directory)).filter((path) =>
    skills.some((skill) => path.startsWith(`${skill}/`)) || path === 'LICENSE-BeautifulRust');
  const stale = current.filter((path) => !upstream.files.has(path));
  const changed = [];
  for (const [path, content] of upstream.files) {
    // Git may check out Markdown with CRLF on Windows.
    const existing = current.includes(path) ? await readFile(join(directory, path), 'utf8') : null;
    if (existing?.replaceAll('\r\n', '\n') !== content.toString('utf8')) changed.push(path);
  }
  if (!changed.length && !stale.length) return false;

  // Finish downloading and validating before touching the installed skills.
  for (const path of changed) {
    const destination = join(directory, path);
    await mkdir(dirname(destination), { recursive: true });
    await writeFile(destination, upstream.files.get(path));
  }
  for (const path of stale) await rm(join(directory, path));
  await writeFile(readmePath, readme.replace(sourceRecord, `\`${upstream.sha}\` (retrieved ${date}).`));
  return true;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    if (process.argv.length > 3) throw new Error('Usage: node scripts/update-beautifulrust.mjs [revision]');
    const upstream = await fetchUpstream(process.argv[2]);
    const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
    const changed = await updateSkills(root, upstream);
    console.log(changed ? `Updated BeautifulRust to ${upstream.sha}` : 'BeautifulRust skills are already current.');
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
