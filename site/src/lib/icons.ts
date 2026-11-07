// The icons on the landing page: one for each language and each way to install, and the GitHub logo.

import {
	siApachegroovy,
	siBazel,
	siCmake,
	siCplusplus,
	siCss,
	siDart,
	siDocker,
	siDotnet,
	siElixir,
	siGithub,
	siGnubash,
	siGo,
	siGraphql,
	siHcl,
	siHomebrew,
	siHtml5,
	siJavascript,
	siJson,
	siKotlin,
	siLua,
	siMake,
	siMarkdown,
	siMdx,
	siNixos,
	siOcaml,
	siOpenjdk,
	siPhp,
	siPython,
	siRuby,
	siRust,
	siScala,
	siSwift,
	siToml,
	siTypescript,
	siXml,
	siYaml,
} from 'simple-icons';

/**
 * An icon on a 24 by 24 grid. Simple Icons are filled shapes in a brand color. The few drawn here by hand are outlines
 * with no color, the way generic icons usually look.
 */
export interface Icon {
	path: string;
	style: 'fill' | 'stroke';
	/** The brand color, adjusted to stay visible on the dark and the light page. Missing for the generic icons. */
	color?: { dark: string; light: string };
}

/** Returns the red, green and blue of a color written as `#rrggbb` or `rrggbb`, each from 0 to 255. */
function rgb(hex: string): number[] {
	const digits = hex.replace(/^#/, '');
	return [0, 2, 4].map((i) => parseInt(digits.slice(i, i + 2), 16));
}

// The page backgrounds.
// <block same-as="site/src/styles/theme.css#/:root/--sl-color-black" same-as-pattern="#[0-9a-f]{6}">
const darkPage = rgb('#0e0e10');
// </block>
// <block same-as="site/src/styles/theme.css#/:root[data-theme='light']/--sl-color-black" same-as-pattern="#[0-9a-f]{6}">
const lightPage = rgb('#ffffff');
// </block>

/** The WCAG relative luminance of an sRGB color, from 0 for black to 1 for white. */
function luminance(rgb: number[]): number {
	const [r, g, b] = rgb.map((c) => {
		const s = c / 255;
		return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
	});
	return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(a: number[], b: number[]): number {
	const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
	return (hi + 0.05) / (lo + 0.05);
}

/**
 * Returns `hex` mixed toward `toward` just enough to reach a 3:1 contrast with `page`, the minimum for an icon in
 * WCAG. Many brand colors are black or pale yellow, which would vanish on one of the two pages.
 */
function visible(hex: string, page: number[], toward: number[]): string {
	const color = rgb(hex);
	for (let t = 0; t <= 1; t += 0.05) {
		const mixed = color.map((c, i) => Math.round(c + (toward[i] - c) * t));
		if (contrast(mixed, page) >= 3) {
			return `rgb(${mixed.join(' ')})`;
		}
	}
	return `rgb(${toward.join(' ')})`;
}

const fill = (icon: { path: string; hex: string }): Icon => ({
	path: icon.path,
	style: 'fill',
	color: { dark: visible(icon.hex, darkPage, lightPage), light: visible(icon.hex, lightPage, darkPage) },
});

// Simple Icons has no logo for these, so they get a generic picture of what they are.
const database: Icon = {
	path: 'M4 6c0-1.7 3.6-3 8-3s8 1.3 8 3-3.6 3-8 3-8-1.3-8-3Zm0 0v12c0 1.7 3.6 3 8 3s8-1.3 8-3V6M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3',
	style: 'stroke',
};
const terminal: Icon = {
	path: 'M4 5h16a1 1 0 0 1 1 1v12a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1Zm3 4 3 3-3 3m5 0h5',
	style: 'stroke',
};
const codeFile: Icon = {
	path: 'M14 3H6a1 1 0 0 0-1 1v16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8Zm0 0v5h5M10 12l-2 2.5 2 2.5m4-5 2 2.5-2 2.5',
	style: 'stroke',
};

// Keys are the names in the README's list of languages. A name missing here fails the build, in `languageIcon`.
const languageIcons: Record<string, Icon> = {
	Bash: fill(siGnubash),
	'C#': fill(siDotnet),
	'C/C++': fill(siCplusplus),
	CMake: fill(siCmake),
	CSS: fill(siCss),
	Dart: fill(siDart),
	Dockerfile: fill(siDocker),
	Elixir: fill(siElixir),
	Go: fill(siGo),
	GraphQL: fill(siGraphql),
	Groovy: fill(siApachegroovy),
	HCL: fill(siHcl),
	HTML: fill(siHtml5),
	JSON: fill(siJson),
	// Oracle's Java logo isn't in Simple Icons. OpenJDK is the open source Java.
	Java: fill(siOpenjdk),
	JavaScript: fill(siJavascript),
	Kotlin: fill(siKotlin),
	Lean: codeFile,
	Lua: fill(siLua),
	MDX: fill(siMdx),
	Makefile: fill(siMake),
	Markdown: fill(siMarkdown),
	Nix: fill(siNixos),
	OCaml: fill(siOcaml),
	PHP: fill(siPhp),
	'Protocol Buffers': codeFile,
	Python: fill(siPython),
	Ruby: fill(siRuby),
	Rust: fill(siRust),
	SQL: database,
	Scala: fill(siScala),
	// Starlark has no logo. It is the language of Bazel's build files.
	Starlark: fill(siBazel),
	Swift: fill(siSwift),
	TOML: fill(siToml),
	TypeScript: fill(siTypescript),
	XML: fill(siXml),
	YAML: fill(siYaml),
};

/** Returns the icon of a language in the README's list. Throws if it has none, so a new language can't ship without one. */
export function languageIcon(language: string): Icon {
	const icon = languageIcons[language];
	if (!icon) {
		throw new Error(`no icon for the language "${language}": add one to src/lib/icons.ts`);
	}
	return icon;
}

// Keys are the tab names of the install commands in src/lib/repo.ts.
const installIcons: Record<string, Icon> = {
	Homebrew: fill(siHomebrew),
	Shell: terminal,
	// Simple Icons leaves out Microsoft's logos, so this is the four panes drawn by hand, in Windows blue.
	Windows: {
		path: 'M3 3h8.5v8.5H3Zm9.5 0H21v8.5h-8.5ZM3 12.5h8.5V21H3Zm9.5 0H21V21h-8.5Z',
		style: 'fill',
		color: { dark: visible('0078d4', darkPage, lightPage), light: visible('0078d4', lightPage, darkPage) },
	},
	Python: fill(siPython),
	Cargo: fill(siRust),
};

/** Returns the icon of an install tab. Throws if it has none. */
export function installIcon(label: string): Icon {
	const icon = installIcons[label];
	if (!icon) {
		throw new Error(`no icon for the install tab "${label}": add one to src/lib/icons.ts`);
	}
	return icon;
}

/** The GitHub logo, in the color of the text around it. */
export const githubIcon: Icon = { path: siGithub.path, style: 'fill' };
