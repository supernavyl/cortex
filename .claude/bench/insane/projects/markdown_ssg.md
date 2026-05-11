Build a Markdown static site generator project in Rust.

Layout:

```
src/
  main.rs           - CLI entry: build, watch, serve subcommands
  lib.rs            - public API
  config.rs         - site config (toml-loaded)
  page.rs           - Page struct + frontmatter parsing
  parser.rs         - Markdown → HTML conversion (pulldown-cmark wrapper)
  builder.rs        - walks src/, applies templates, writes to dist/
  template.rs       - simple {{var}} template engine (no Handlebars dep)
tests/
  build.rs          - integration tests: feed a fixture site dir, check dist/ output
```

`Cargo.toml` deps:
- `clap = { version = "4", features = ["derive"] }`
- `serde = { version = "1", features = ["derive"] }`
- `toml = "0.8"`
- `pulldown-cmark = "0.13"`
- `walkdir = "2"`
- `anyhow = "1"`
- `thiserror = "1"`

Subcommands (clap derive):
- `ssg build [--config site.toml]` — process content/ → dist/
- `ssg new <name>` — scaffold a new post in content/posts/

Content layout (input):
```
content/
  index.md          - YAML frontmatter: {title, layout}
  posts/
    *.md            - YAML frontmatter: {title, date, layout, tags}
templates/
  default.html      - {{title}}, {{content}}, {{site.name}}, {{site.url}} placeholders
  post.html         - extends default; {{post.date}}, {{post.title}}, etc.
static/             - copied verbatim to dist/
```

Page parsing:
- Detect YAML frontmatter delimited by `---` lines at file start
- If absent, treat whole file as content with empty frontmatter
- Frontmatter parsed via serde + a serde-yaml dep — OR just hand-parse `key: value`
  for the simple cases (title, date, layout, tags). Pick one and document it.
- Markdown → HTML via pulldown-cmark with `Options::ENABLE_TABLES | ENABLE_FOOTNOTES`

Template engine (hand-rolled, ~50 LOC):
- `{{var}}`         — replace with value of `var` in context
- `{{site.name}}`   — nested path
- `{{#each posts}}...{{/each}}`  — block iteration
- No conditionals needed for v0

Required tests:
- `test_parse_frontmatter_present`
- `test_parse_no_frontmatter_treats_as_body`
- `test_markdown_to_html_paragraph` — `"hello"` → `"<p>hello</p>"`
- `test_template_substitution_simple` — `{{x}}` with x="hi" → `"hi"`
- `test_template_iteration` — `{{#each items}}{{name}}-{{/each}}` over 3 items
- `test_build_creates_dist_dir` — fixture site builds, check `dist/index.html` exists
- `test_static_dir_copied_verbatim`

`cargo check` clean, `cargo test` passes.
