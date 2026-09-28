# Ferese website and handbook

The public site is static HTML and CSS. The handbook is generated from the
project's Markdown guides, so website and repository documentation stay in sync.
There are no client frameworks, external fonts, or tracking scripts.

## Local preview

```sh
python3 -m venv .site-venv
.site-venv/bin/pip install -r site/requirements.txt
.site-venv/bin/python scripts/build-site.py
python3 -m http.server 8080 --bind 127.0.0.1 --directory build/site
```

Open http://localhost:8080 or http://localhost:8080/docs/. Rebuild after editing
site sources or Markdown guides, then refresh the browser.

- `site/index.html` and `site/styles.css`: homepage and shared visual design.
- `site/content/index.md`: handbook introduction.
- `docs/{installation,configuration,desktop-widgets,notifications,locking,screen-sharing}.md`:
  source guides.
- `scripts/build-site.py`: renders Markdown, resolves links, generates section
  navigation, previous/next links, and the search index.
- `site/app.js`: handbook search and code copying. Reading and navigation work
  without JavaScript. Search is local to the browser, with no third-party service.

## GitHub Pages

Select **Settings → Pages → Source → GitHub Actions**. The Pages workflow
publishes site and documentation changes pushed to `main`, or can run manually.
Expected URL: https://ferese-wm.github.io/ferese/.

Local links are relative, including documentation search, so publishing under
the `/ferese/` project path works. Screenshots are copied from `docs/images`
during the build; the generated `build/site` directory is ignored by Git.

See [GitHub's custom workflow documentation](https://docs.github.com/en/pages/getting-started-with-github-pages/using-custom-workflows-with-github-pages)
for deployment setup.
