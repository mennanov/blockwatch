# blockwatch website

The landing page and the docs site, built with [Astro](https://astro.build) and
[Starlight](https://starlight.astro.build).

```shell
npm install
npm run dev      # serve on http://localhost:4321 and reload on changes
npm run build    # write the static site to dist/
```

The docs pages are not stored here. Each build copies them from `../README.md` and `../docs/` with
`scripts/sync-docs.mjs`, into `src/content/docs/docs/`, which git ignores. Edit the files in `../docs/`. To publish a
new file, add it to `../docs/` and to the sidebar in `astro.config.mjs`. Every Markdown file in `../docs/` is
published, and the build fails if one is missing from the sidebar. Notes and drafts go in `../.claude/` instead.

The landing page, `src/pages/index.astro`, reads the version, the languages, the validators and the agent prompt from
`../README.md` and `../Cargo.toml` at build time.

The site is published at <https://blockwatch-cli.pages.dev>, a Cloudflare Pages project named `blockwatch-cli`.
`../.github/workflows/site.yml` builds and deploys it on every push to `main` that changes the site or a file it reads.
A pull request only builds it. If the build fails, nothing is deployed and the old site stays live. The deploy uses
the `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` repository secrets. The token needs only the "Cloudflare Pages:
Edit" permission.
