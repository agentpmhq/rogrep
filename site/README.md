# rogrep.sh site

The project website and documentation use [Eleventy](https://www.11ty.dev/) with a local [Pagefind](https://pagefind.app/) search index.

~~~sh
npm ci
npm run dev     # generate CLI help, then serve with live reload
npm run build   # CLI help + Eleventy + Pagefind
npm test        # crawl metadata, routes, links, and anchors
~~~

CLI pages are generated from Clap `--help` output. `site/scripts/generate-cli.mjs` contains the explicit command manifest and fails when it differs from the binary. Canonical query documentation and the bundled skill are imported from the repository at build time rather than copied.
