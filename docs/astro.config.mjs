// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import starlightLinksValidator from 'starlight-links-validator';
import { readFileSync } from 'node:fs';

// Подсветка блоков ```outry (формат .outry, reference/outry-format).
const outryGrammar = JSON.parse(readFileSync(new URL('./src/outry.tmLanguage.json', import.meta.url), 'utf8'));

// GitHub Pages: https://1rowvy.github.io/outry/
export default defineConfig({
	site: 'https://1rowvy.github.io',
	base: '/outry',
	// Относительные ссылки в Markdown (`../cli/`) работают одинаково на любой странице.
	trailingSlash: 'always',
	integrations: [
		starlight({
			title: 'Outry',
			description: 'Executable API specs that live in your repository.',
			logo: { light: './src/assets/logo-light.svg', dark: './src/assets/logo-dark.svg' },
			head: [
				{ tag: 'link', attrs: { rel: 'preconnect', href: 'https://fonts.googleapis.com' } },
				{ tag: 'link', attrs: { rel: 'preconnect', href: 'https://fonts.gstatic.com', crossorigin: true } },
				{
					tag: 'link',
					attrs: { rel: 'stylesheet', href: 'https://fonts.googleapis.com/css2?family=Space+Grotesk:wght@500;600;700&display=swap' },
				},
			],
			favicon: '/favicon.svg',
			defaultLocale: 'root',
			locales: {
				root: { label: 'English', lang: 'en' },
				ru: { label: 'Русский', lang: 'ru' },
			},
			social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/1rowvy/outry' }],
			editLink: { baseUrl: 'https://github.com/1rowvy/outry/edit/master/docs/' },
			lastUpdated: true,
			customCss: ['./src/styles/custom.css'],
			expressiveCode: { shiki: { langs: [outryGrammar] } },
			// Ломаем сборку на битых внутренних ссылках, в том числе между языками.
			plugins: [starlightLinksValidator()],
			sidebar: [
				{
					label: 'Start here',
					translations: { ru: 'Начало' },
					items: [
						{ slug: 'getting-started' },
						{ slug: 'example' },
						{ slug: 'install' },
					],
				},
				{
					label: 'Guides',
					translations: { ru: 'Руководства' },
					items: [
						{ slug: 'guides/chains-and-assertions' },
						{ slug: 'guides/variables' },
						{ slug: 'guides/secrets' },
						{ slug: 'guides/ci' },
						{ slug: 'guides/import-go' },
						{ slug: 'guides/desktop-app' },
						{ slug: 'guides/editors' },
						{ slug: 'guides/request-format' },
					],
				},
				{
					label: 'Reference',
					translations: { ru: 'Справочник' },
					items: [
						{ slug: 'reference/outry-format' },
						{ slug: 'reference/cli' },
						{ slug: 'reference/env-toml' },
						{ slug: 'reference/expressions' },
					],
				},
			],
		}),
	],
});
