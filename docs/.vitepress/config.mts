import { readdirSync, readFileSync } from 'node:fs'
import { join, relative } from 'node:path'
import { defineConfig } from 'vitepress'

const base = '/mailtriage/'

export default defineConfig({
  title: 'mailtriage',
  description:
    'Local email classification and attention queries for humans and agents',
  base,
  cleanUrls: true,
  themeConfig: {
    nav: [
      { text: 'Guide', link: '/guide/introduction', activeMatch: '^/guide/' },
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
          text: 'Guide',
          items: [
            { text: 'Introduction', link: '/guide/introduction' },
            { text: 'Install', link: '/guide/install' },
            { text: 'Guided setup', link: '/guide/setup' },
            { text: 'Manual setup', link: '/guide/manual-setup' },
            { text: 'Configuration', link: '/guide/configuration' },
            { text: 'Provider', link: '/guide/provider' },
            { text: 'Background service', link: '/guide/service' },
            { text: 'Updates', link: '/guide/updates' },
            { text: 'Himalaya', link: '/guide/himalaya' },
            { text: 'Daily use', link: '/guide/daily-use' },
            { text: 'Categories', link: '/guide/categories' },
            { text: 'Filing', link: '/guide/filing' },
            { text: 'Provider check', link: '/guide/provider-check' },
            { text: 'Tray', link: '/guide/tray' },
            { text: 'Reference', link: '/guide/reference' },
          ],
        },
      ],
      '/agents/': [
        {
          text: 'Agents',
          items: [{ text: 'Agent guide', link: '/agents/' }],
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
