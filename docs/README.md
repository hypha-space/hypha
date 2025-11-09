# Hypha Documentation

This directory contains the source for Hypha's documentation website, built with [Zola](https://www.getzola.org/) and deployed to Cloudflare.

## Local Development

### Prerequisites

- [Zola](https://www.getzola.org/documentation/getting-started/installation/) v0.21.0 or later

### Building Locally

```bash
# Navigate to docs directory
cd docs

# Build the site
zola build

# Serve locally with live reload
zola serve
```

The documentation will be available at `http://127.0.0.1:1111`.

## Project Structure

```
docs/
├── config.toml              # Zola configuration
├── content/                 # Markdown content
│   ├── _index.md           # Homepage
│   ├── getting-started.md  # Getting started guide
│   ├── architecture/       # Architecture documentation
│   ├── deployment/         # Deployment guides
│   └── contributing.md     # Contributing guide
├── static/                 # Static assets (images, favicon, etc.)
├── themes/                 # Custom theme
│   └── hypha/
│       ├── templates/      # HTML templates
│       └── static/         # Theme assets (CSS, JS)
└── public/                 # Generated site (ignored by git)
```

## Writing Documentation

### Adding a New Page

1. Create a new markdown file in `content/`:

```bash
# Simple page
touch content/new-page.md

# Page in a section
mkdir -p content/section-name
touch content/section-name/_index.md
touch content/section-name/new-page.md
```

2. Add frontmatter to the markdown file:

```markdown
+++
title = "Page Title"
description = "Page description"
weight = 1
+++

# Content here
```

3. Write your content using [CommonMark](https://commonmark.org/) markdown

### Adding a New Section

1. Create a directory in `content/`
2. Add an `_index.md` file with section metadata:

```markdown
+++
title = "Section Title"
description = "Section description"
sort_by = "weight"
+++

Section overview content
```

### Frontmatter Options

- `title`: Page title (required)
- `description`: Page description
- `weight`: Sort order (lower numbers appear first)
- `template`: Custom template to use
- `draft`: Set to `true` to exclude from builds

## Theme Customization

The custom theme is located in `themes/hypha/`:

- **Templates**: `themes/hypha/templates/`
  - `base.html`: Base template with header/footer
  - `index.html`: Homepage template
  - `page.html`: Individual page template
  - `section.html`: Section listing template

- **Styles**: `themes/hypha/static/css/style.css`
  - Minimal, responsive design
  - Dark mode support via `prefers-color-scheme`

## Deployment

Documentation is automatically deployed to Cloudflare when changes are pushed to the `main` branch.

### Manual Deployment

If you have Cloudflare credentials configured:

```bash
# Build the site
cd docs
zola build

# Deploy with Wrangler
cd ..
wrangler deploy
```

### Required Secrets

Configure these secrets in your GitHub repository:

- `CLOUDFLARE_API_TOKEN`: Cloudflare API token with Workers deploy permissions
- `CLOUDFLARE_ACCOUNT_ID`: Your Cloudflare account ID

## Contributing

See the main [CONTRIBUTING.md](../CONTRIBUTING.md) for general contribution guidelines.

For documentation-specific contributions:

1. Make changes to markdown files in `content/`
2. Test locally with `zola serve`
3. Submit a pull request
4. Documentation will be deployed automatically on merge

## Resources

- [Zola Documentation](https://www.getzola.org/documentation/)
- [CommonMark Spec](https://commonmark.org/)
- [Cloudflare Workers](https://workers.cloudflare.com/)
