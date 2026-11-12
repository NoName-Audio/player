import { readFile } from 'node:fs/promises';

const source = process.env.GITHUB_REPOSITORY;
const target = 'NoName-Audio/player-plugins';
const tag = process.env.RELEASE_TAG;
const branch = 'automation/attachment-targets';
const path = 'docs/attachment-targets.md';
const api = process.env.GITHUB_API_URL || 'https://api.github.com';

async function request(repo, endpoint, { method = 'GET', body, missing = false } = {}) {
  const response = await fetch(`${api}/repos/${repo}/${endpoint}`, {
    method,
    headers: {
      Authorization: `Bearer ${repo === target ? process.env.PLUGINS_TOKEN : process.env.API_TOKEN}`,
      Accept: 'application/vnd.github+json',
      'X-GitHub-Api-Version': '2022-11-28',
      'Content-Type': 'application/json',
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (missing && response.status === 404) return null;
  if (!response.ok) throw new Error(`${method} ${repo}/${endpoint}: ${response.status} ${await response.text()}`);
  return response.status === 204 ? null : response.json();
}

// Publication time, not tag spelling or dispatch order, defines the current release.
async function isCurrentRelease() {
  let newest;
  for (let page = 1; ; page++) {
    const releases = await request(source, `releases?per_page=100&page=${page}`);
    for (const release of releases) {
      if (!release.draft && release.published_at &&
          (!newest || Date.parse(release.published_at) > Date.parse(newest.published_at) ||
            (release.published_at === newest.published_at && release.id > newest.id))) newest = release;
    }
    if (releases.length < 100) break;
  }
  if (!newest) throw new Error('No published source release exists; documentation must follow publication.');
  if (newest.tag_name !== tag) {
    console.log(`Skipping stale release ${tag}; newest published release is ${newest.tag_name}.`);
    return false;
  }
  return true;
}

async function publish() {
  if (!source || !tag || !process.env.API_TOKEN || !process.env.PLUGINS_TOKEN) {
    throw new Error('GITHUB_REPOSITORY, RELEASE_TAG, API_TOKEN and PLUGINS_TOKEN are required.');
  }
  if (!await isCurrentRelease()) return;
  const content = await readFile(process.env.DOCS_PATH, 'utf8');
  const repo = await request(target, '');
  const base = repo.default_branch;
  if (branch === base) throw new Error('Documentation branch must not be the default branch.');
  const baseRef = await request(target, `git/ref/heads/${encodeURIComponent(base)}`);
  const baseCommit = await request(target, `git/commits/${baseRef.object.sha}`);
  const existing = await request(target, `contents/${path}?ref=${encodeURIComponent(base)}`, { missing: true });
  const pulls = await request(target, `pulls?state=open&head=NoName-Audio:${branch}&base=${encodeURIComponent(base)}`);
  if (existing && Buffer.from(existing.content, 'base64').toString('utf8') === content) {
    if (!await isCurrentRelease()) return;
    for (const pull of pulls) await request(target, `pulls/${pull.number}`, { method: 'PATCH', body: { state: 'closed' } });
    console.log('Published documentation already matches the default branch.');
    return;
  }
  const ref = await request(target, `git/ref/heads/${branch}`, { missing: true });
  const tree = await request(target, 'git/trees', {
    method: 'POST',
    body: { base_tree: baseCommit.tree.sha, tree: [{ path, mode: '100644', type: 'blob', content }] },
  });
  // Include the previous automation head as a parent so ref updates are fast-forward
  // only. A concurrent writer causes an explicit failure rather than lost changes.
  const parents = [...new Set([baseRef.object.sha, ...(ref ? [ref.object.sha] : [])])];
  const commit = await request(target, 'git/commits', {
    method: 'POST', body: { message: `docs: attachment targets for player ${tag}`, tree: tree.sha, parents },
  });
  if (!await isCurrentRelease()) return;
  if (ref) {
    await request(target, `git/refs/heads/${branch}`, { method: 'PATCH', body: { sha: commit.sha, force: false } });
  } else {
    await request(target, 'git/refs', { method: 'POST', body: { ref: `refs/heads/${branch}`, sha: commit.sha } });
  }
  const details = {
    title: `docs: attachment targets for player ${tag}`,
    body: `Generated from the exact published player tag [${tag}](https://github.com/${source}/releases/tag/${encodeURIComponent(tag)}).\n\nOnly \`${path}\` is updated. Review and merge this PR; automation never writes the default branch.`,
  };
  if (pulls.length) {
    await request(target, `pulls/${pulls[0].number}`, { method: 'PATCH', body: details });
  } else {
    await request(target, 'pulls', { method: 'POST', body: { ...details, head: branch, base } });
  }
}

await publish();
