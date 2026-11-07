// Copies the repository's Markdown docs into Starlight's content folder.
//
// The docs stay where they are, so GitHub shows them as before and there is only one copy to edit. The copies in
// src/content/docs/docs/ are in .gitignore. Starlight needs a title in the front matter and site URLs in links, so
// each copy gets both here.

import { mkdir, readdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = fileURLToPath(new URL('../../', import.meta.url));
const docsDir = path.join(repoRoot, 'docs');
const outDir = fileURLToPath(new URL('../src/content/docs/', import.meta.url));
const githubBlob = 'https://github.com/mennanov/blockwatch/blob/main/';
const githubEdit = 'https://github.com/mennanov/blockwatch/edit/main/';

/**
 * Returns the published pages: the README and every Markdown file in docs/, keyed by their repository path, such as
 * `docs/cli.md`. Each has the URL path of its page, such as `docs/cli`, and a title when the file's own heading
 * isn't the one to show. Notes and drafts don't belong in docs/, because everything there is published.
 */
export async function listPages() {
	const pages = new Map([['README.md', { slug: 'docs', title: 'Overview' }]]);
	const files = await readdir(docsDir, { recursive: true });
	for (const file of files.filter((f) => f.endsWith('.md')).sort()) {
		const source = path.posix.join('docs', file.split(path.sep).join('/'));
		// A README.md is the page of its folder, as on GitHub.
		const slug = source.replace(/(\/README)?\.md$/, '');
		pages.set(source, { slug });
	}
	return pages;
}

/**
 * Turns a link target written in `source` into a URL that works on the site.
 *
 * Takes the published `pages` and the target as written, such as `../cli.md#exit-codes`. Returns the site URL of a
 * published page, a GitHub URL for any other file in the repository, and an external URL or a `#anchor` unchanged.
 */
function rewriteTarget(pages, source, target) {
	if (/^[a-z][a-z0-9+.-]*:/i.test(target) || target.startsWith('#')) {
		return target;
	}
	const hashAt = target.indexOf('#');
	const file = hashAt === -1 ? target : target.slice(0, hashAt);
	const anchor = hashAt === -1 ? '' : target.slice(hashAt);
	const repoPath = path.posix.normalize(path.posix.join(path.posix.dirname(source), file));
	const page = pages.get(repoPath);
	if (page) {
		return `/${page.slug}/${anchor}`;
	}
	return githubBlob + repoPath + anchor;
}

/**
 * Returns the Markdown of `source`, ready for Starlight: no first-level heading, no badge lines, and every link
 * rewritten with `rewriteTarget`.
 *
 * Lines inside fenced code blocks are left alone. They hold examples, and a link in an example is not a real link.
 */
function convert(pages, source, markdown) {
	let fence = null;
	const out = [];
	for (const line of markdown.split('\n')) {
		const fenceMatch = line.match(/^\s*(`{3,}|~{3,})/);
		if (fence) {
			if (fenceMatch && fenceMatch[1][0] === fence[0] && fenceMatch[1].length >= fence.length) {
				fence = null;
			}
			out.push(line);
			continue;
		}
		if (fenceMatch) {
			fence = fenceMatch[1];
			out.push(line);
			continue;
		}
		// Starlight prints the title as the page heading, so the file's own heading would show twice.
		if (/^# /.test(line) || /^\[!\[/.test(line)) {
			continue;
		}
		out.push(line.replace(/\]\(([^)\s]+)\)/g, (_, target) => `](${rewriteTarget(pages, source, target)})`));
	}
	return out.join('\n');
}

/** Returns the text of the first `# ` heading in `markdown`, without backticks. */
function headingOf(markdown) {
	const heading = markdown.match(/^# (.+)$/m);
	if (!heading) {
		throw new Error('no first-level heading');
	}
	return heading[1].replaceAll('`', '');
}

/**
 * Rewrites every published page into src/content/docs/ and returns the pages, as `listPages` does. Throws if a file
 * has no heading.
 */
export async function syncDocs() {
	const pages = await listPages();
	await rm(path.join(outDir, 'docs'), { recursive: true, force: true });
	for (const [source, page] of pages) {
		const markdown = await readFile(path.join(repoRoot, source), 'utf8');
		let title;
		try {
			title = page.title ?? headingOf(markdown);
		} catch (error) {
			throw new Error(`${source}: ${error.message}`);
		}
		const frontMatter = [
			'---',
			`# Made from ${source} by site/scripts/sync-docs.mjs. Edit that file instead.`,
			`title: ${JSON.stringify(title)}`,
			`editUrl: ${JSON.stringify(githubEdit + source)}`,
			'---',
			'',
		].join('\n');
		const target = path.join(outDir, `${page.slug}.md`);
		await mkdir(path.dirname(target), { recursive: true });
		await writeFile(target, frontMatter + convert(pages, source, markdown));
	}
	return pages;
}

/** Returns the absolute paths a file watcher needs: the README and the docs folder, so a new file is seen too. */
export function sourcePaths() {
	return [path.join(repoRoot, 'README.md'), docsDir];
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
	await syncDocs();
}
