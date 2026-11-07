// Facts for the landing page, read from the repository at build time. The page shows the same list of languages,
// validators and version as the README and Cargo.toml, with no copy to keep in sync by hand.

// A `?raw` import, not a file read, because the bundled page no longer sits next to this file.
import readme from '../../../README.md?raw';
import cargoToml from '../../../Cargo.toml?raw';

/** Returns the README text between the start tag of the block `name` and the next end tag. Throws if it is missing. */
function readmeBlock(name: string): string {
	const start = readme.indexOf(`<block name="${name}"`);
	if (start === -1) {
		throw new Error(`README.md has no block "${name}"`);
	}
	const end = readme.indexOf('</block>', start);
	return readme.slice(readme.indexOf('\n', start) + 1, end);
}

/** The license in Cargo.toml, such as `MIT`. */
export const license = cargoToml.match(/^license = "([^"]+)"/m)![1];

/** The blockwatch version in Cargo.toml, such as `0.18.0`. */
export const version = cargoToml.match(/^version = "([^"]+)"/m)![1];

/**
 * The languages in the README's supported-languages list, in the README's order. Each is the name only, such as
 * `Python`, without the extensions in parentheses.
 */
export const languages: string[] = [...readmeBlock('supported-grammar').matchAll(/^- (.+?)(?: \(|$)/gm)].map(
	(match) => match[1],
);

/** One row of the README's validator table. */
export interface Validator {
	name: string;
	description: string;
}

/** The validators in the README's table, in the README's order. */
export const validators: Validator[] = [
	...readmeBlock('available-validators').matchAll(/^\|\s*\[`([a-z-]+)`\]\([^)]*\)\s*\|\s*([^|]+?)\s*\|/gm),
].map((match) => ({ name: match[1], description: match[2] }));

/** Returns the text inside the first fenced code block in `markdown`. Throws if there is none. */
function codeIn(markdown: string): string {
	const code = markdown.match(/```\w*\n([\s\S]*?)\n```/);
	if (!code) {
		throw new Error(`README.md: no code block in:\n${markdown}`);
	}
	return code[1];
}

/** The prompt that the README tells users to paste into their coding agent. */
export const agentPrompt = codeIn(readmeBlock('agent-prompt')).replace(/\n/g, ' ');

/** A way to install blockwatch, from the README's install commands. */
export interface Install {
	/** A short name for the tab, such as `Homebrew`. */
	label: string;
	command: string;
}

// The README explains each command in a comment line, too long for a tab. The tab name comes from the program the
// command runs instead.
const installLabels: Record<string, string> = {
	brew: 'Homebrew',
	curl: 'Shell',
	powershell: 'Windows',
	pipx: 'Python',
	cargo: 'Cargo',
};

/**
 * The install commands in the README, in the README's order, without their comments. Throws for a command whose
 * program has no tab name.
 */
export const installs: Install[] = codeIn(readmeBlock('install'))
	.split('\n')
	.filter((line) => line !== '' && !line.startsWith('#'))
	.map((line) => {
		const command = line.replace(/\s+# .*$/, '');
		const label = installLabels[command.split(' ')[0]];
		if (!label) {
			throw new Error(`README.md: no tab name for the install command "${command}", add one in src/lib/repo.ts`);
		}
		return { label, command };
	});

/** The commands that install the Claude Code plugin, from the README, in order. */
export const pluginInstall: string[] = codeIn(readmeBlock('plugin-install')).split('\n');
