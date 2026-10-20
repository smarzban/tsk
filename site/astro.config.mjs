// @ts-check
import { defineConfig } from 'astro/config';
import sitemap from '@astrojs/sitemap';
import starlight from '@astrojs/starlight';
import { ExpressiveCodeTheme } from '@astrojs/starlight/expressive-code';

import { rehypeKbd } from './src/plugins/rehype-kbd.mjs';

// Code blocks stay mono like the landing page: ink, a dimmer ink, and weight. No hue.
function codeTheme(name, type, palette) {
  return new ExpressiveCodeTheme({
    name,
    type,
    colors: {
      'editor.background': palette.bg,
      'editor.foreground': palette.ink,
    },
    tokenColors: [
      { scope: ['comment', 'punctuation.definition.comment'], settings: { foreground: palette.dim } },
      { scope: ['string', 'string.quoted', 'punctuation.definition.string'], settings: { foreground: palette.ink2 } },
      { scope: ['constant.other.option', 'variable.parameter'], settings: { foreground: palette.ink2 } },
      { scope: ['keyword', 'support.function', 'entity.name.function', 'support.type.property-name'], settings: { foreground: palette.ink, fontStyle: 'bold' } },
    ],
  });
}

const codeDark = codeTheme('tsk-dark', 'dark', { bg: '#171719', ink: '#ebe7dc', ink2: '#aca89e', dim: '#66635c' });
const codeLight = codeTheme('tsk-light', 'light', { bg: '#faf8f3', ink: '#151517', ink2: '#4d4b46', dim: '#8b887f' });

export default defineConfig({
  site: 'https://gettsk.sh',
  // Keep inter-tag whitespace: the landing copy relies on spaces between text and inline tags.
  compressHTML: false,
  markdown: {
    rehypePlugins: [rehypeKbd],
  },
  integrations: [
    sitemap({
      filter: (page) => !page.endsWith('.md'),
    }),
    starlight({
      title: 'tsk',
      description: 'A task board for you and your agents, in your terminal.',
      favicon: '/icon-dark.svg',
      social: [
        {
          icon: 'github',
          label: 'GitHub',
          href: 'https://github.com/smarzban/tsk',
        },
      ],
      editLink: {
        baseUrl: 'https://github.com/smarzban/tsk/edit/main/site/',
      },
      lastUpdated: true,
      customCss: ['./src/styles/starlight.css'],
      expressiveCode: {
        themes: [codeDark, codeLight],
        useStarlightDarkModeSwitch: true,
        useStarlightUiThemeColors: false,
        defaultProps: { wrap: true, preserveIndent: true },
        styleOverrides: {
          borderRadius: '4px',
          borderColor: 'var(--sl-color-gray-5)',
          codeFontFamily: 'var(--sl-font)',
          codeFontSize: '0.82rem',
          codeLineHeight: '1.7',
          codePaddingBlock: '1rem',
          codePaddingInline: '1.25rem',
          frames: {
            shadowColor: 'transparent',
            editorBackground: 'var(--sl-color-bg-nav)',
            terminalBackground: 'var(--sl-color-bg-nav)',
            inlineButtonForeground: 'var(--sl-color-gray-2)',
            inlineButtonBackground: 'var(--sl-color-gray-5)',
            inlineButtonBorder: 'var(--sl-color-gray-5)',
            tooltipSuccessBackground: 'var(--sl-color-accent)',
            tooltipSuccessForeground: '#121214',
          },
        },
      },
      components: {
        Head: './src/components/DocsHead.astro',
        PageTitle: './src/components/DocsPageTitle.astro',
        SiteTitle: './src/components/DocsTitle.astro',
        ThemeSelect: './src/components/ThemeSelect.astro',
        SocialIcons: './src/components/DocsLinks.astro',
      },
      sidebar: [
        {
          label: 'Start here',
          items: [
            { label: 'Overview', slug: 'docs' },
            { label: 'Install', slug: 'docs/install' },
          ],
        },
        {
          label: 'Use tsk',
          items: [
            { label: 'Board', slug: 'docs/board' },
            { label: 'Dispatch', slug: 'docs/dispatch' },
            { label: 'Keys', slug: 'docs/keys' },
            { label: 'Capture', slug: 'docs/capture' },
            { label: 'Task page', slug: 'docs/task-page' },
            { label: 'Steps', slug: 'docs/steps' },
          ],
        },
        {
          label: 'Reference',
          items: [
            { label: 'tsk for agents', slug: 'docs/agents' },
            { label: 'Coordinator skill', slug: 'docs/coordinator' },
            { label: 'CLI', slug: 'docs/cli' },
            { label: 'Storage', slug: 'docs/storage' },
          ],
        },
      ],
      head: [
        {
          // Paint the saved theme before anything renders. Must run before theme.js,
          // whose fallback would otherwise overwrite the saved choice.
          tag: 'script',
          content: `(function(){var k='tsk-theme';var t=null;try{t=localStorage.getItem(k)||localStorage.getItem('starlight-theme');}catch(e){}if(t!=='light'&&t!=='dark')t='dark';document.documentElement.dataset.theme=t;document.documentElement.style.colorScheme=t;try{localStorage.setItem(k,t);localStorage.setItem('starlight-theme',t);}catch(e){}})();`,
        },
        {
          tag: 'script',
          attrs: { src: '/theme.js' },
        },
        {
          tag: 'script',
          attrs: { src: '/docs-copy.js' },
        },
        {
          tag: 'link',
          attrs: { rel: 'preconnect', href: 'https://fonts.googleapis.com' },
        },
        {
          tag: 'link',
          attrs: {
            rel: 'preconnect',
            href: 'https://fonts.gstatic.com',
            crossorigin: true,
          },
        },
        {
          tag: 'link',
          attrs: {
            rel: 'stylesheet',
            href: 'https://fonts.googleapis.com/css2?family=JetBrains+Mono:ital,wght@0,400;0,500;0,700;0,800;1,400&display=swap',
          },
        },
        {
          tag: 'link',
          attrs: {
            rel: 'icon',
            href: '/icon-dark.svg',
            type: 'image/svg+xml',
            media: '(prefers-color-scheme: dark)',
          },
        },
        {
          tag: 'link',
          attrs: {
            rel: 'icon',
            href: '/icon-light.svg',
            type: 'image/svg+xml',
            media: '(prefers-color-scheme: light)',
          },
        },
        {
          tag: 'link',
          attrs: { rel: 'apple-touch-icon', href: '/apple-touch-icon.png' },
        },
        {
          tag: 'link',
          attrs: {
            rel: 'apple-touch-icon',
            href: '/apple-touch-icon-dark.png',
            media: '(prefers-color-scheme: dark)',
          },
        },
        {
          tag: 'link',
          attrs: {
            rel: 'apple-touch-icon',
            href: '/apple-touch-icon-light.png',
            media: '(prefers-color-scheme: light)',
          },
        },
      ],
    }),
  ],
});
