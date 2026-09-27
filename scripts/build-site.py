#!/usr/bin/env python3
"""Build Ferese's static website and handbook from the project Markdown docs."""
from pathlib import Path
from html import escape
import json
import re
import shutil
from urllib.parse import urlsplit
from markdown_it import MarkdownIt

ROOT = Path(__file__).resolve().parent.parent
OUTPUT = ROOT / 'build' / 'site'
REPO = 'https://github.com/borngraced/ferese'
PAGES = [
    ('index', 'Introduction', ROOT / 'site/content/index.md'),
    ('installation', 'Installation', ROOT / 'docs/installation.md'),
    ('configuration', 'Configuration', ROOT / 'docs/configuration.md'),
    ('desktop-widgets', 'Desktop widgets', ROOT / 'docs/desktop-widgets.md'),
    ('notifications', 'Notifications', ROOT / 'docs/notifications.md'),
]
md = MarkdownIt('commonmark', {'html': False}).enable('table')
OUTPUT.mkdir(parents=True, exist_ok=True)
for name in ('index.html', 'styles.css', 'app.js'):
    shutil.copy2(ROOT / 'site' / name, OUTPUT / name)
(OUTPUT / 'assets').mkdir(exist_ok=True)
for name in ('ferese-desktop.png', 'ferese-monochrome.png'):
    shutil.copy2(ROOT / 'docs/images' / name, OUTPUT / 'assets' / name)
shutil.copy2(ROOT / 'packaging/icons/ferese.svg', OUTPUT / 'assets/ferese.svg')
(OUTPUT / 'docs').mkdir(exist_ok=True)
(OUTPUT / '.nojekyll').touch()
search_index = []


def rewrite_link(href, source):
    url = urlsplit(href)
    if url.scheme or url.netloc or not url.path:
        return href
    basename = Path(url.path).stem
    if url.path.endswith('.md') and basename in {page[0] for page in PAGES}:
        return f'{basename}.html' + (f'#{url.fragment}' if url.fragment else '')
    destination = (source.parent / url.path).resolve().relative_to(ROOT)
    return f'{REPO}/blob/main/{destination}' + (f'#{url.fragment}' if url.fragment else '')


for page_number, (slug, title, source) in enumerate(PAGES):
    tokens = md.parse(source.read_text())
    toc, ids, sections = [], set(), []
    current = None
    for i, token in enumerate(tokens):
        if token.type == 'heading_open':
            heading = tokens[i + 1].content
            base = re.sub(r'[^\w\s-]', '', heading.lower()).strip()
            base = re.sub(r'[\s_]+', '-', base)
            anchor = base
            suffix = 1
            while anchor in ids:
                anchor = f'{base}-{suffix}'
                suffix += 1
            ids.add(anchor)
            token.attrSet('id', anchor)
            if token.tag != 'h1':
                toc.append((anchor, heading))
            current = {'page': title, 'heading': heading, 'url': f'{slug}.html#{anchor}', 'text': ''}
            sections.append(current)
        elif current and token.type in ('inline', 'fence', 'code_block'):
            current['text'] += token.content + ' '
        for child in token.children or []:
            if child.type == 'link_open':
                child.attrSet('href', rewrite_link(child.attrGet('href'), source))
    search_index.extend(sections)
    body = md.renderer.render(tokens, md.options, {})
    body = body.replace('<table>', '<div class="table-scroll" tabindex="0" role="region" aria-label="Reference table"><table>').replace('</table>', '</table></div>')
    navigation = ''.join(f'<a href="{s}.html"' + (' aria-current="page"' if s == slug else '') + f'>{escape(t)}</a>' for s, t, _ in PAGES)
    contents = ''.join(f'<a href="#{anchor}">{escape(heading)}</a>' for anchor, heading in toc)
    pager = ''
    if page_number:
        prev = PAGES[page_number - 1]
        pager += f'<a href="{prev[0]}.html"><small>PREVIOUS</small>← {escape(prev[1])}</a>'
    if page_number + 1 < len(PAGES):
        nxt = PAGES[page_number + 1]
        pager += f'<a href="{nxt[0]}.html"><small>NEXT</small>{escape(nxt[1])} →</a>'
    doc = f'''<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="theme-color" content="#10151c"><title>{escape(title)} — Ferese handbook</title><meta name="description" content="{escape(title)} in the Ferese handbook: setup, configuration, and guides for your Wayland desktop.">
<link rel="icon" href="../assets/ferese.svg" type="image/svg+xml"><link rel="stylesheet" href="../styles.css"><script src="../app.js" defer></script></head>
<body class="docs-page"><a class="skip" href="#main">Skip to content</a>
<header class="site-header"><div class="header-inner"><a class="wordmark" href="../" aria-label="Ferese home"><svg viewBox="0 0 24 24" aria-hidden="true"><path d="M3 3h6v18H3zM11 3h10v6H11zM11 11h7v6h-7z"/></svg>ferese</a><nav aria-label="Main navigation"><a href="../">Overview</a><a href="./" aria-current="page">Documentation</a><a href="{REPO}">GitHub <svg class="link-arrow" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><path d="M7 17 17 7M7 7h10v10"/></svg></a></nav></div></header>
<div class="docs-layout"><aside class="docs-sidebar"><div class="docs-search" hidden><label class="sr-only" for="docs-search">Search the handbook</label><input id="docs-search" type="search" placeholder="Search the handbook…" autocomplete="off" aria-controls="search-results"><div class="search-results" id="search-results" hidden></div><p class="sr-only" id="search-status" role="status"></p></div><details open><summary>FERESE HANDBOOK</summary><nav aria-label="Documentation">{navigation}</nav></details><div class="sidebar-links"><a href="{REPO}/issues">Report an issue <svg class="link-arrow" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><path d="M7 17 17 7M7 7h10v10"/></svg></a><a href="../">Back to Ferese ←</a></div></aside>
<main class="docs-article" id="main"><div class="breadcrumb"><a href="./">Handbook</a><span aria-hidden="true">/</span><span>{escape(title)}</span></div><article class="prose">{body}</article><a class="edit-link" href="{REPO}/blob/main/{source.relative_to(ROOT)}">View this guide on GitHub <svg class="link-arrow" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><path d="M7 17 17 7M7 7h10v10"/></svg></a><nav class="docs-pager" aria-label="Previous and next guide">{pager}</nav></main>
<aside class="docs-toc" aria-label="On this page">{'<p>ON THIS PAGE</p><nav>' + contents + '</nav>' if contents else ''}</aside></div>
<footer class="site-footer page-width"><a class="wordmark" href="../">ferese</a><p>A beautiful, fluid Wayland desktop.</p><a href="{REPO}">Built in the open <svg class="link-arrow" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><path d="M7 17 17 7M7 7h10v10"/></svg></a></footer></body></html>'''
    (OUTPUT / 'docs' / f'{slug}.html').write_text(doc)
(OUTPUT / 'docs/search-index.json').write_text(json.dumps(search_index, ensure_ascii=False))
print(f'Built website and {len(PAGES)} handbook pages at {OUTPUT}')
