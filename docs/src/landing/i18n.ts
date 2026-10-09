// Тексты лендинга. Код в примерах общий для обоих языков, переводятся только подписи.

export type Lang = 'en' | 'ru';

const en = {
	htmlTitle: 'Outry — executable API specs that live in your repository',
	description:
		'Describe every endpoint in plain .outry files next to the code, run them from the terminal, editor, desktop app or CI — and see which requests no longer match your Go code.',
	nav: { docs: 'Docs', format: 'Format', cli: 'CLI', github: 'GitHub' },
	hero: {
		eyebrow: 'open source · one Rust core · MIT',
		title: ['API specs', 'that run.'],
		lead: 'Describe every endpoint in plain .outry files next to the code. Run them from the terminal, your editor, the desktop app or CI — and when the code changes, Outry shows which requests no longer match it.',
		start: 'Get started',
		example: 'Example project',
		copy: 'copy',
		copied: 'copied',
	},
	stats: [
		['0', 'collections to export'],
		['1', 'engine for CLI, app, editors, CI'],
		['3', 'Go routers read: chi · gin · net/http'],
		['∞', 'requests per run, one Login()'],
	],
	anatomy: {
		kicker: '01 / format',
		title: 'One file is the docs, the request and the test.',
		lead: 'A .outry request reads like the HTTP it sends. Hover a part to see what it does.',
		parts: [
			['name', 'Name', 'Requests are named like functions and unique per project. No imports.'],
			['only', 'Environments', '`only` limits where a request may run — directly or through a call.'],
			['call', 'Calls', '`Login()` is a request. Called ten times, sent once per run.'],
			['body', 'Body', 'JSON-like literals with expressions, functions like `uuid()` and calls inside.'],
			['expect', 'Checks', 'Every line is a boolean. All run; failures print both sides.'],
			['shape', 'Shapes', '`matches Order` validates structure — named shapes or JSON Schema.'],
			['save', 'Save', 'Typed values for later requests, persisted outside the repo.'],
		],
	},
	drift: {
		kicker: '02 / sync',
		title: 'It notices when the code moves.',
		lead: 'Outry parses your Go service with tree-sitter and compares every request with its handler: method, path, body fields and types, query, headers, middleware, response type. Drift fails CI on the exact line — with a fix.',
		codeTab: 'internal/api/users.go',
		specTab: 'api/v1/users/post.outry',
		steps: ['a struct field is renamed', 'outry check', '--fix rewrites the request'],
		routers: 'chi · gin · net/http (Go 1.22 patterns) · group prefixes · Mount across packages · middleware',
	},
	flow: {
		kicker: '03 / composition',
		title: 'Requests compose like functions.',
		lead: 'A flow is a scenario. Calls resolve dependencies, share one cookie jar and one call cache, and every hop is traced with its response and timing.',
		cached: 'cached',
		poll: 'poll every 1s',
		total: 'total',
	},
	arch: {
		kicker: '04 / architecture',
		title: 'One engine everywhere.',
		lead: 'Parser, evaluator, HTTP, variables, secrets and Go import live in outry-core. Every surface is a thin wrapper, so what passes on your machine passes in CI.',
		core: 'outry-core',
		coreSub: 'Rust · parser · eval · reqwest · tree-sitter',
		nodes: [
			['CLI', 'run · check · fmt · import'],
			['Desktop', 'Tauri 2 · auto-update'],
			['VS Code', 'LSP + response view'],
			['Any editor', 'Neovim · Helix · Zed'],
			['CI', 'annotations · JSON Lines'],
		],
	},
	vars: {
		kicker: '05 / variables',
		title: 'Every value has exactly one origin.',
		lead: 'Variables resolve through a fixed chain — and `outry vars` tells you which layer each value came from. Secrets stay in the system keychain or `OUTRY_*`.',
		layers: [
			['--var', 'override for one run'],
			['save', 'captured from responses'],
			['OUTRY_<NAME>', 'process environment, CI secrets'],
			['env.toml [env.X] → [vars]', 'committed with the code'],
			['keyring', 'system keychain, queried lazily'],
		],
	},
	ci: {
		kicker: '06 / CI',
		title: 'Your spec is your test suite.',
		lead: 'Static checks first — syntax, call graph, variables per environment, formatting, Go drift — without sending a byte. Then run against the real service.',
	},
	docs: {
		kicker: '07 / documentation',
		title: 'Read the manual.',
		lead: 'Guides walk through real tasks; the reference covers every keyword, flag and config key.',
		groups: ['Start here', 'Guides', 'Reference'],
		cli: 'CLI at a glance',
		commands: [
			['outry run <paths|names>', 'send requests, run checks'],
			['outry check', 'static analysis, envs, fmt, Go drift'],
			['outry fmt', 'canonical style, like gofmt'],
			['outry import go', 'generate & diff requests from routes'],
			['outry vars -e prod', 'every variable and its source'],
			['outry secret set token', 'store a secret in the keychain'],
			['outry lsp', 'language server for any editor'],
			['outry convert', '.http → .outry'],
		],
	},
	cta: {
		title: 'Put your API next to its code.',
		lead: 'Linux, macOS and Windows. One static binary, no runtime.',
	},
	footer: { license: 'MIT licensed', made: 'Docs', release: 'Releases' },
};

type Strings = typeof en;

const ru: Strings = {
	htmlTitle: 'Outry — исполняемые спецификации API, которые живут в репозитории',
	description:
		'Опишите каждый эндпоинт в текстовых .outry-файлах рядом с кодом, запускайте их из терминала, редактора, приложения или CI — и смотрите, какие запросы больше не соответствуют коду на Go.',
	nav: { docs: 'Документация', format: 'Формат', cli: 'CLI', github: 'GitHub' },
	hero: {
		eyebrow: 'open source · одно ядро на Rust · MIT',
		title: ['Спецификации API,', 'которые запускаются.'],
		lead: 'Опишите каждый эндпоинт в текстовых .outry-файлах рядом с кодом. Запускайте их из терминала, редактора, приложения или CI — а когда код изменится, Outry покажет, какие запросы ему больше не соответствуют.',
		start: 'Начать',
		example: 'Пример проекта',
		copy: 'копировать',
		copied: 'скопировано',
	},
	stats: [
		['0', 'коллекций для экспорта'],
		['1', 'движок для CLI, приложения, редакторов и CI'],
		['3', 'роутера Go: chi · gin · net/http'],
		['∞', 'запросов за прогон — и один Login()'],
	],
	anatomy: {
		kicker: '01 / формат',
		title: 'Один файл — это документация, запрос и тест.',
		lead: 'Запрос .outry читается как HTTP, который он отправляет. Наведите на часть, чтобы увидеть, что она делает.',
		parts: [
			['name', 'Имя', 'Запросы называются как функции и уникальны в проекте. Импорты не нужны.'],
			['only', 'Окружения', '`only` ограничивает, где запрос может выполняться — напрямую или через вызов.'],
			['call', 'Вызовы', '`Login()` — это запрос. Вызван десять раз — отправлен один раз за прогон.'],
			['body', 'Тело', 'Литералы как в JSON, с выражениями, функциями вроде `uuid()` и вызовами.'],
			['expect', 'Проверки', 'Каждая строка — условие. Выполняются все; при ошибке видны обе стороны.'],
			['shape', 'Формы', '`matches Order` проверяет структуру — именованной формой или JSON Schema.'],
			['save', 'Сохранение', 'Типизированные значения для следующих запросов, хранятся вне репозитория.'],
		],
	},
	drift: {
		kicker: '02 / синхронизация',
		title: 'Он замечает, когда код меняется.',
		lead: 'Outry разбирает Go-сервис через tree-sitter и сравнивает каждый запрос с его обработчиком: метод, путь, поля и типы тела, query, заголовки, middleware, тип ответа. Расхождение роняет CI на нужной строке — вместе с исправлением.',
		codeTab: 'internal/api/users.go',
		specTab: 'api/v1/users/post.outry',
		steps: ['поле структуры переименовано', 'outry check', '--fix правит запрос'],
		routers: 'chi · gin · net/http (паттерны Go 1.22) · префиксы групп · Mount между пакетами · middleware',
	},
	flow: {
		kicker: '03 / композиция',
		title: 'Запросы складываются как функции.',
		lead: 'flow — это сценарий. Вызовы сами подтягивают зависимости, делят одну банку cookies и один кеш вызовов, а каждый шаг виден в трассе с ответом и временем.',
		cached: 'из кеша',
		poll: 'poll каждую 1s',
		total: 'итого',
	},
	arch: {
		kicker: '04 / архитектура',
		title: 'Один движок везде.',
		lead: 'Парсер, вычислитель, HTTP, переменные, секреты и импорт из Go живут в outry-core. Всё остальное — тонкие обёртки, поэтому то, что проходит у вас, проходит и в CI.',
		core: 'outry-core',
		coreSub: 'Rust · парсер · eval · reqwest · tree-sitter',
		nodes: [
			['CLI', 'run · check · fmt · import'],
			['Приложение', 'Tauri 2 · автообновление'],
			['VS Code', 'LSP + просмотр ответа'],
			['Любой редактор', 'Neovim · Helix · Zed'],
			['CI', 'аннотации · JSON Lines'],
		],
	},
	vars: {
		kicker: '05 / переменные',
		title: 'У каждого значения ровно один источник.',
		lead: 'Переменные ищутся по фиксированной цепочке, а `outry vars` показывает, из какого слоя пришло каждое значение. Секреты — в системном хранилище паролей или в `OUTRY_*`.',
		layers: [
			['--var', 'переопределение на один прогон'],
			['save', 'сохранено из ответов'],
			['OUTRY_<NAME>', 'переменные окружения, секреты CI'],
			['env.toml [env.X] → [vars]', 'лежит в репозитории'],
			['keyring', 'хранилище паролей, по запросу'],
		],
	},
	ci: {
		kicker: '06 / CI',
		title: 'Спецификация и есть тесты.',
		lead: 'Сначала статические проверки — синтаксис, граф вызовов, переменные каждого окружения, форматирование, расхождения с Go — без единого запроса. Потом прогон на живом сервисе.',
	},
	docs: {
		kicker: '07 / документация',
		title: 'Читайте документацию.',
		lead: 'Руководства разбирают реальные задачи; справочник описывает каждое ключевое слово, флаг и ключ конфига.',
		groups: ['Начало', 'Руководства', 'Справочник'],
		cli: 'CLI коротко',
		commands: [
			['outry run <пути|имена>', 'отправить запросы, выполнить проверки'],
			['outry check', 'статический анализ, окружения, fmt, Go'],
			['outry fmt', 'канонический стиль, как gofmt'],
			['outry import go', 'создать и сверить запросы по роутам'],
			['outry vars -e prod', 'все переменные и их источники'],
			['outry secret set token', 'положить секрет в хранилище'],
			['outry lsp', 'языковой сервер для любого редактора'],
			['outry convert', '.http → .outry'],
		],
	},
	cta: {
		title: 'Держите API рядом с его кодом.',
		lead: 'Linux, macOS и Windows. Один статический бинарник, без рантайма.',
	},
	footer: { license: 'Лицензия MIT', made: 'Документация', release: 'Релизы' },
};

export const strings: Record<Lang, Strings> = { en, ru };
