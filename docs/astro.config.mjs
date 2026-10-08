// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import starlightLinksValidator from 'starlight-links-validator';
import { readFileSync } from 'node:fs';

// Подсветка блоков ```routy (формат .routy, reference/routy-format).
const routyGrammar = JSON.parse(readFileSync(new URL('./src/routy.tmLanguage.json', import.meta.url), 'utf8'));

// GitHub Pages: https://1rowvy.github.io/routy/
export default defineConfig({
	site: 'https://1rowvy.github.io',
	base: '/routy',
	// Относительные ссылки в Markdown (`../cli/`) работают одинаково на любой странице.
	trailingSlash: 'always',
	integrations: [
		starlight({
			title: 'Routy',
			description: 'API client where requests are plain .http files in your repo.',
			logo: { src: './src/assets/logo.svg' },
			favicon: '/favicon.svg',
			defaultLocale: 'root',
			locales: {
				root: { label: 'English', lang: 'en' },
				ru: { label: 'Русский', lang: 'ru' },
			},
			social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/1rowvy/routy' }],
			editLink: { baseUrl: 'https://github.com/1rowvy/routy/edit/master/docs/' },
			lastUpdated: true,
			customCss: ['./src/styles/custom.css'],
			expressiveCode: { shiki: { langs: [routyGrammar] } },
			// Ломаем сборку на битых внутренних ссылках, в том числе между языками.
			plugins: [starlightLinksValidator()],
			sidebar: [
				{
					label: 'Start here',
					translations: { ru: 'Начало' },
					items: [
						{ slug: 'getting-started' },
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
						{ slug: 'guides/request-format' },
					],
				},
				{
					label: 'Reference',
					translations: { ru: 'Справочник' },
					items: [
						{ slug: 'reference/routy-format' },
						{ slug: 'reference/cli' },
						{ slug: 'reference/env-toml' },
						{ slug: 'reference/expressions' },
					],
				},
			],
		}),
	],
});
