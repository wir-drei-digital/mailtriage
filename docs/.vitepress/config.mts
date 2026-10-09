import { readdirSync, readFileSync } from 'node:fs'
import { join, relative } from 'node:path'
import { defineConfig } from 'vitepress'

const base = '/mailtriage/'

export default defineConfig({
  title: 'mailtriage',
  description:
    'Set up mailtriage, review your email and manage optional filing',
  base,
  cleanUrls: true,
  head: [['link', { rel: 'icon', type: 'image/png', href: `${base}favicon.png` }]],
  themeConfig: {
    logo: { src: '/logo.png', alt: '' },
    nav: [
      { text: 'Guide', link: '/guide/introduction', activeMatch: '^/guide/' },
      { text: 'Reference', link: '/reference/', activeMatch: '^/reference/' },
      { text: 'Agents', link: '/agents/', activeMatch: '^/agents/' },
      {
        text: 'Development',
        link: '/development/',
        activeMatch: '^/development/',
      },
    ],
    socialLinks: [
      { icon: 'github', link: 'https://github.com/wir-drei-digital/mailtriage' },
    ],
    sidebar: {
      '/guide/': [
        {
          text: 'Get started',
          items: [
            { text: 'What is mailtriage?', link: '/guide/introduction' },
            { text: 'Install', link: '/guide/install' },
            { text: 'Set up your mailbox', link: '/guide/setup' },
          ],
        },
        {
          text: 'Use mailtriage',
          items: [
            { text: 'Daily use', link: '/guide/daily-use' },
            { text: 'Change categories', link: '/guide/categories' },
            { text: 'Use the tray app', link: '/guide/tray' },
            { text: 'Run in the background', link: '/guide/service' },
            { text: 'File mail into folders', link: '/guide/filing' },
            { text: 'Updates', link: '/guide/updates' },
          ],
        },
        {
          text: 'Help and settings',
          items: [
            { text: 'Troubleshooting', link: '/guide/troubleshooting' },
            { text: 'Configuration', link: '/guide/configuration' },
            { text: 'API keys and models', link: '/guide/provider' },
            { text: 'Mail provider compatibility', link: '/guide/provider-check' },
            { text: 'Himalaya versions', link: '/guide/himalaya' },
            { text: 'Manual setup', link: '/guide/manual-setup' },
            { text: 'Errors and local data', link: '/guide/reference' },
            { text: 'Technical reference', link: '/reference/' },
          ],
        },
      ],
      '/reference/': [
        {
          text: 'Technical reference',
          items: [
            { text: 'Overview', link: '/reference/' },
            { text: 'Installation', link: '/reference/install' },
            { text: 'Setup', link: '/reference/setup' },
            { text: 'Configuration', link: '/reference/configuration' },
            { text: 'Provider and keys', link: '/reference/provider' },
            { text: 'Sync and queries', link: '/reference/daily-use' },
            { text: 'Background service', link: '/reference/service' },
            { text: 'Filing', link: '/reference/filing' },
            { text: 'Tray', link: '/reference/tray' },
            { text: 'Updates', link: '/reference/updates' },
            { text: 'Errors and local data', link: '/guide/reference' },
          ],
        },
      ],
      '/agents/': [
        {
          text: 'Agents',
          items: [
            { text: 'Agent guide', link: '/agents/' },
            { text: 'Command reference', link: '/agents/reference' },
          ],
        },
      ],
      '/development/': [
        {
          text: 'Development',
          items: [
            { text: 'Overview', link: '/development/' },
            { text: 'Service API', link: '/development/service-api' },
            { text: 'Providers', link: '/development/providers' },
            { text: 'Releases', link: '/development/releases' },
            { text: 'Verification', link: '/development/verification' },
            { text: 'Mail provider tests', link: '/development/mail-provider-check' },
          ],
        },
      ],
    },
    search: { provider: 'local' },
    editLink: {
      pattern:
        'https://github.com/wir-drei-digital/mailtriage/edit/main/docs/:path',
      text: 'Edit this page on GitHub',
    },
    outline: [2, 3],
    footer: { message: 'mailtriage by wirdrei.digital' },
  },
  // VitePress fails the build on a link to a missing page, but not on a
  // missing `#anchor`; this does, for every link between the site's pages.
  buildEnd(siteConfig) {
    checkAnchors(siteConfig.outDir)
  },
})

function htmlFiles(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name)
    if (entry.isDirectory()) return htmlFiles(path)
    return entry.name.endsWith('.html') ? [path] : []
  })
}

/** The built file a site URL path (under `base`) is served from. */
function fileOf(pathname: string): string {
  const route = pathname.slice(base.length)
  if (route === '' || route.endsWith('/')) return `${route}index.html`
  return route.endsWith('.html') ? route : `${route}.html`
}

function checkAnchors(outDir: string) {
  const origin = 'https://site.invalid'
  const ids = new Map<string, Set<string>>()
  const links: [string, string][] = []
  for (const file of htmlFiles(outDir)) {
    const page = relative(outDir, file)
    const html = readFileSync(file, 'utf8')
    ids.set(page, new Set([...html.matchAll(/\sid="([^"]+)"/g)].map((m) => m[1])))
    const route = page.replace(/(^|\/)index\.html$/, '$1').replace(/\.html$/, '')
    for (const m of html.matchAll(/\shref="([^"]*#[^"]*)"/g)) {
      links.push([page, new URL(m[1], `${origin}${base}${route}`).href])
    }
  }
  const missing = links.filter(([, href]) => {
    const url = new URL(href)
    if (url.origin !== origin || !url.pathname.startsWith(base) || !url.hash) {
      return false
    }
    const known = ids.get(fileOf(url.pathname))
    return !known?.has(decodeURIComponent(url.hash.slice(1)))
  })
  if (missing.length > 0) {
    const list = missing.map(([page, href]) => `${page}: ${href.slice(origin.length)}`)
    throw new Error(`links to missing anchors:\n  ${list.join('\n  ')}`)
  }
}
