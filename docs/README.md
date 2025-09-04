# Omni Documentation

This directory contains the official documentation for Omni, built using [Docusaurus](https://docusaurus.io/).

## Overview

The documentation is designed primarily for **IT teams** who need to deploy and manage Omni in production environments. It covers:

- **Quick Start**: Get Omni running in under 10 minutes
- **Production Deployment**: Enterprise-ready setup guides
- **Configuration**: Database, security, and data source setup
- **Operations**: Monitoring, maintenance, and troubleshooting
- **Architecture**: Technical deep-dive for system planning

## Development

### Prerequisites

- **Node.js** 18+ 
- **npm** or **yarn**

### Local Development

```bash
# Install dependencies
npm install

# Start development server
npm start
```

This starts a local server at http://localhost:3000 with hot reload.

### Building

```bash
# Build static site
npm run build

# Serve built site locally
npm run serve
```

The built site is generated in the `build/` directory.

## Documentation Structure

```
docs/
├── docs/                           # Main documentation content
│   ├── intro.md                   # Landing page
│   ├── getting-started/           # Quick start guides
│   │   ├── overview.md
│   │   ├── system-requirements.md
│   │   └── docker-deployment.md
│   ├── deployment/                # Production deployment
│   │   └── production-setup.md
│   ├── configuration/             # Configuration guides
│   ├── operations/                # Operations and maintenance
│   │   └── monitoring.md
│   ├── integrations/              # Data source integrations
│   └── architecture/              # Technical architecture
│       └── overview.md
├── blog/                          # Blog posts and updates
├── src/                          # Custom components and pages
├── static/                       # Static assets
└── docusaurus.config.ts          # Site configuration
```

## Contributing to Documentation

### Writing Guidelines

1. **Audience**: Write for IT teams deploying and managing Omni
2. **Structure**: Use clear headings and step-by-step instructions
3. **Code Examples**: Include practical, copy-paste ready examples
4. **Screenshots**: Add visual aids for complex procedures
5. **Links**: Cross-reference related documentation

### Adding New Pages

1. Create markdown file in appropriate `docs/` subdirectory
2. Add frontmatter with title and sidebar position
3. Update `sidebars.ts` if needed for navigation
4. Test locally with `npm start`

### Style Guidelines

- Use **bold** for UI elements and important concepts
- Use `code` for commands, file paths, and variables
- Use > blockquotes for important notes
- Use tables for structured information
- Include practical examples for all procedures

## Deployment

The documentation can be deployed to various hosting platforms:

### GitHub Pages

```bash
# Deploy to gh-pages branch
GIT_USER=<username> npm run deploy
```

### Static Hosting

Build and upload the `build/` directory to any static hosting service:
- **Netlify**: Connect GitHub repo for automatic deployments
- **Vercel**: Import project for automatic deployments  
- **AWS S3**: Upload build directory to S3 bucket
- **GitHub Pages**: Use GitHub Actions for automated deployment

## Maintenance

### Regular Updates

- **Content Review**: Update procedures when Omni features change
- **Link Checking**: Verify all internal and external links
- **Screenshot Updates**: Keep UI screenshots current
- **Version Alignment**: Ensure docs match current Omni version

### Monitoring

- **Analytics**: Track page views and user paths
- **Feedback**: Monitor GitHub issues for documentation feedback
- **Search**: Review search queries to identify content gaps

## Support

- **Issues**: [Report documentation issues](https://github.com/omnihq/omni/issues)
- **Discussions**: [Ask questions](https://github.com/omnihq/omni/discussions)
- **Contributing**: See main project [CONTRIBUTING.md](../CONTRIBUTING.md)
