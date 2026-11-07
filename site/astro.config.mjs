// @ts-check
import { defineConfig } from 'astro/config';
import path from 'node:path';
import starlight from '@astrojs/starlight';
import { sourcePaths, syncDocs } from './scripts/sync-docs.mjs';

// Every published page must be in this list. The build fails otherwise, so a new file in docs/ can't end up on the
// site with no way to reach it.
const sidebar = [
	{
		label: 'Start here',
		items: [
			{ label: 'Overview', slug: 'docs' },
			{ label: 'AI agents', slug: 'docs/agents' },
			{ label: 'CI and Git hooks', slug: 'docs/ci' },
		],
	},
	{
		label: 'Validators',
		items: [
			{ label: 'All validators', slug: 'docs/validators' },
			{ label: 'affects', slug: 'docs/validators/affects' },
			{ label: 'same-as', slug: 'docs/validators/same-as' },
			{ label: 'keep-sorted', slug: 'docs/validators/keep-sorted' },
			{ label: 'keep-unique', slug: 'docs/validators/keep-unique' },
			{ label: 'line-pattern', slug: 'docs/validators/line-pattern' },
			{ label: 'line-count', slug: 'docs/validators/line-count' },
			{ label: 'check-ai', slug: 'docs/validators/check-ai' },
			{ label: 'check-lua', slug: 'docs/validators/check-lua' },
		],
	},
	{
		label: 'Reference',
		items: [
			{ label: 'CLI and config file', slug: 'docs/cli' },
			{ label: 'Symbols', slug: 'docs/symbols' },
		],
	},
];

/**
 * Copies the repository's docs into Starlight's content folder before each build and dev server start. In dev, it
 * copies them again when a file in docs/ or the README changes, so an edit to docs/cli.md shows up without a restart.
 * Fails if a page is missing from the sidebar.
 *
 * @returns {import('astro').AstroIntegration}
 */
function repositoryDocs() {
	const sync = async () => {
		const pages = await syncDocs();
		const inSidebar = new Set(sidebar.flatMap((group) => group.items.map((item) => item.slug)));
		const missing = [...pages].filter(([, page]) => !inSidebar.has(page.slug)).map(([source]) => source);
		if (missing.length > 0) {
			throw new Error(`add these docs to the sidebar in site/astro.config.mjs: ${missing.join(', ')}`);
		}
	};
	return {
		name: 'repository-docs',
		hooks: {
			'astro:config:setup': sync,
			'astro:server:setup': ({ server, logger }) => {
				const [readme, docsDir] = sourcePaths();
				server.watcher.add([readme, docsDir]);
				const onChange = async (file) => {
					if (file === readme || (file.startsWith(docsDir + path.sep) && file.endsWith('.md'))) {
						try {
							await sync();
							logger.info(`copied the docs again after a change to ${file}`);
						} catch (error) {
							logger.error(error.message);
						}
					}
				};
				for (const event of ['add', 'change', 'unlink']) {
					server.watcher.on(event, onChange);
				}
			},
		},
	};
}

export default defineConfig({
	site: 'https://blockwatch-cli.pages.dev',
	integrations: [
		repositoryDocs(),
		starlight({
			title: 'blockwatch',
			description: 'A linter that keeps code, docs and configs that must change together in sync.',
			logo: { src: './src/assets/logo.svg', replacesTitle: false },
			favicon: '/favicon.svg',
			social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/mennanov/blockwatch' }],
			customCss: [
				'@fontsource-variable/geist',
				'@fontsource-variable/geist-mono',
				'./src/styles/theme.css',
			],
			sidebar,
		}),
	],
});
