// Мини-подсветка для лендинга: на выходе HTML, по строке на <span class="ln">, чтобы строки можно было
// выделять аннотациями и анимациями. Полноценная грамматика живёт в outry.tmLanguage.json.

type Rule = [RegExp, string];

const OUTRY: Rule[] = [
	[/^\/\/.*/, 'c'],
	[/^#.*/, 'c'],
	[/^"(?:[^"\\]|\\.)*"/, 's'],
	[/^\b(?:GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)\b/, 'm'],
	[
		/^\b(?:expect|save|params|headers|query|body|flow|shape|poll|every|for|matches|only|let|fresh|cache|confirm|handler|status|duration|cookies)\b/,
		'k',
	],
	[/^\b(?:string|number|integer|boolean|null|any|true|false)\b/, 't'],
	[/^\b[A-Z][A-Za-z0-9]*(?:\.[A-Z][A-Za-z0-9]*)*(?=\()/, 'f'],
	[/^\b[A-Z][A-Za-z0-9]*(?=:\s*(?:GET|POST|PUT|PATCH|DELETE)\b)/, 'n'],
	[/^\b[A-Z][A-Za-z0-9]*\b/, 'ty'],
	[/^\b\d+(?:\.\d+)?(?:ms|s|m|h)?\b/, 'num'],
	[/^\/[A-Za-z0-9_\-/{}.]*/, 'p'],
	[/^(?:==|!=|<=|>=|&&|\|\||[<>|?=])/, 'o'],
	[/^[A-Za-z_][\w-]*/, 'id'],
	[/^\s+/, ''],
	[/^./, 'pu'],
];

const GO: Rule[] = [
	[/^\/\/.*/, 'c'],
	[/^`[^`]*`/, 's'],
	[/^"(?:[^"\\]|\\.)*"/, 's'],
	[/^\b(?:package|import|func|type|struct|return|if|err|nil|var|const)\b/, 'k'],
	[/^\b(?:string|int|int64|bool|error|float64)\b/, 't'],
	[/^\b[A-Za-z_]\w*(?=\()/, 'f'],
	[/^\b\d+\b/, 'num'],
	[/^[A-Za-z_]\w*/, 'id'],
	[/^\s+/, ''],
	[/^./, 'pu'],
];

const YAML: Rule[] = [
	[/^#.*/, 'c'],
	[/^"(?:[^"\\]|\\.)*"/, 's'],
	[/^\$\{\{[^}]*\}\}/, 'f'],
	[/^[\w-]+(?=:)/, 'k'],
	[/^\boutry\b/, 'n'],
	[/^--?[\w-]+/, 'm'],
	[/^[A-Za-z_][\w./@-]*/, 'id'],
	[/^\s+/, ''],
	[/^./, 'pu'],
];

const LANGS = { outry: OUTRY, go: GO, yaml: YAML };

const esc = (s: string) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');

function line(src: string, rules: Rule[]): string {
	let out = '';
	let rest = src;
	while (rest.length) {
		for (const [re, cls] of rules) {
			const m = re.exec(rest);
			if (!m) continue;
			const text = m[0];
			// Интерполяция ${…} внутри строк .outry: внутренность подсвечивается как код.
			if (cls === 's' && rules === OUTRY && text.includes('${')) {
				out += text.replace(/\$\{([^}]*)\}|([^$]+|\$)/g, (_, expr, plain) =>
					expr !== undefined
						? `<span class="tk-o">\${</span>${line(expr, rules)}<span class="tk-o">}</span>`
						: `<span class="tk-s">${esc(plain)}</span>`,
				);
			} else {
				out += cls ? `<span class="tk-${cls}">${esc(text)}</span>` : esc(text);
			}
			rest = rest.slice(text.length);
			break;
		}
	}
	return out;
}

export function highlight(code: string, lang: keyof typeof LANGS): string {
	return code
		.replace(/^\n|\n$/g, '')
		.split('\n')
		.map((l, i) => `<span class="ln" data-line="${i + 1}">${line(l, LANGS[lang]) || ' '}</span>`)
		.join('');
}
