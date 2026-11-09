/**
 * The attributes of the Umami analytics script tag. The landing page and the Starlight docs each write their own head,
 * so both read the tag from here.
 *
 * `data-domains` limits counting to the live site, so `astro dev` and the preview deploys on pages.dev don't add
 * visits.
 */
export const umamiScript = {
	defer: true,
	src: 'https://cloud.umami.is/script.js',
	'data-website-id': '68da5d03-e197-4d99-9746-2a1f02216553',
	'data-domains': 'blockwatch.sh',
};
