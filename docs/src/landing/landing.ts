// Поведение лендинга: появление секций, терминал с набором команды, анимация расхождения с Go,
// подсветка частей запроса, поиск переменной по слоям и фон героя. Всё уважает prefers-reduced-motion.

const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Вызывает `fn`, когда элемент впервые попадает в экран. */
function onVisible(el: Element, fn: () => void, threshold = 0.25) {
	const io = new IntersectionObserver(
		(entries) => {
			if (entries.some((e) => e.isIntersecting)) {
				io.disconnect();
				fn();
			}
		},
		{ threshold },
	);
	io.observe(el);
}

function reveal() {
	document.querySelectorAll('.rv').forEach((el) => {
		if (reduced) return el.classList.add('in');
		onVisible(el, () => el.classList.add('in'), 0.15);
	});
}

function copyButtons() {
	document.querySelectorAll<HTMLElement>('.install').forEach((box) => {
		const btn = box.querySelector<HTMLButtonElement>('.copy')!;
		btn.addEventListener('click', async () => {
			try {
				await navigator.clipboard.writeText(box.dataset.copy ?? '');
				btn.textContent = btn.dataset.done ?? '';
				box.classList.add('done');
				await sleep(1600);
			} finally {
				btn.textContent = btn.dataset.label ?? '';
				box.classList.remove('done');
			}
		});
	});
}

function theme() {
	document.querySelector('.theme')?.addEventListener('click', () => {
		const next = document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark';
		document.documentElement.dataset.theme = next;
		try {
			localStorage.setItem('starlight-theme', next);
		} catch {}
	});
}

async function heroTerminal() {
	const body = document.querySelector<HTMLElement>('[data-typed]');
	if (!body) return;
	const cmd = body.querySelector<HTMLElement>('.cmd')!;
	const typed = cmd.querySelector<HTMLElement>('.typed')!;
	const lines = [...body.querySelectorAll<HTMLElement>('.line:not(.cmd)')];
	const text = cmd.dataset.cmd ?? '';
	if (reduced) {
		typed.textContent = text;
		lines.forEach((l) => l.classList.add('show'));
		return;
	}
	for (;;) {
		typed.textContent = '';
		lines.forEach((l) => l.classList.remove('show'));
		body.classList.remove('done');
		await sleep(700);
		for (const ch of text) {
			typed.textContent += ch;
			await sleep(38 + Math.random() * 50);
		}
		await sleep(350);
		for (const l of lines) {
			l.classList.add('show');
			await sleep(l.classList.contains('sub') ? 110 : 260);
		}
		body.classList.add('done');
		await sleep(6000);
	}
}

function anatomy() {
	const root = document.querySelector<HTMLElement>('.anatomy');
	if (!root) return;
	const items = [...root.querySelectorAll<HTMLElement>('.parts li')];
	const lines = [...root.querySelectorAll<HTMLElement>('.ln')];
	let auto = true;
	let i = 0;
	const select = (part: string) => {
		items.forEach((li) => li.classList.toggle('on', li.dataset.part === part));
		lines.forEach((ln) => ln.classList.toggle('hl', ln.dataset.part === part));
		root.classList.add('focus');
	};
	items.forEach((li) => {
		const pick = () => {
			auto = false;
			select(li.dataset.part!);
		};
		li.addEventListener('mouseenter', pick);
		li.addEventListener('focusin', pick);
		li.tabIndex = 0;
	});
	lines.forEach((ln) =>
		ln.addEventListener('mouseenter', () => {
			if (!ln.dataset.part) return;
			auto = false;
			select(ln.dataset.part);
		}),
	);
	select(items[0].dataset.part!);
	if (reduced) return;
	onVisible(root, async () => {
		while (auto) {
			await sleep(2600);
			if (!auto) break;
			i = (i + 1) % items.length;
			select(items[i].dataset.part!);
		}
	});
}

function drift() {
	const root = document.querySelector<HTMLElement>('.drift');
	if (!root) return;
	if (reduced) {
		root.dataset.step = '3';
		return;
	}
	onVisible(root, async () => {
		for (;;) {
			for (const [step, ms] of [
				['0', 1600],
				['1', 2600],
				['2', 3800],
				['3', 4200],
			] as const) {
				root.dataset.step = step;
				await sleep(ms);
			}
		}
	});
}

function trace() {
	const root = document.querySelector<HTMLElement>('.trace');
	if (!root) return;
	if (reduced) return root.classList.add('run');
	onVisible(root, async () => {
		for (;;) {
			root.classList.add('run');
			await sleep(7000);
			root.classList.remove('run');
			await sleep(500);
		}
	});
}

function vars() {
	const root = document.querySelector<HTMLElement>('.vars');
	if (!root) return;
	const layers = [...root.querySelectorAll<HTMLElement>('.layers li')];
	const rows = [...root.querySelectorAll<HTMLElement>('.vrow')];
	const clear = () => layers.forEach((l) => l.classList.remove('probe', 'hit', 'miss'));
	if (reduced) {
		rows.forEach((r) => r.classList.add('show'));
		return;
	}
	onVisible(root, async () => {
		for (;;) {
			rows.forEach((r) => r.classList.remove('show', 'cur'));
			for (const row of rows) {
				const src = Number(row.dataset.src);
				row.classList.add('show', 'cur');
				clear();
				for (const layer of layers) {
					const n = Number(layer.dataset.layer);
					layer.classList.add('probe');
					await sleep(170);
					if (n === src) {
						layer.classList.replace('probe', 'hit');
						break;
					}
					layer.classList.replace('probe', 'miss');
				}
				if (src === 0) root.classList.add('missing');
				await sleep(1100);
				root.classList.remove('missing');
				row.classList.remove('cur');
			}
			clear();
			await sleep(1800);
		}
	});
}

/** Фон героя: ортогональные «трассы» по сетке, по которым бегут пакеты. */
function field() {
	const canvas = document.querySelector<HTMLCanvasElement>('.field');
	if (!canvas) return;
	const ctx = canvas.getContext('2d')!;
	const STEP = 32;
	let w = 0;
	let h = 0;
	let dpr = 1;
	let ink = '255,255,255';
	type Pkt = { pts: [number, number][]; t: number; speed: number };
	let pkts: Pkt[] = [];

	const route = (): [number, number][] => {
		const cols = Math.ceil(w / STEP);
		const rows = Math.ceil(h / STEP);
		let x = Math.floor(Math.random() * cols);
		let y = Math.floor(Math.random() * rows);
		const pts: [number, number][] = [[x, y]];
		for (let k = 0; k < 4; k++) {
			const len = 2 + Math.floor(Math.random() * 7);
			if (k % 2 === 0) x += Math.random() < 0.5 ? len : -len;
			else y += Math.random() < 0.5 ? len : -len;
			pts.push([x, y]);
		}
		return pts.map(([a, b]) => [a * STEP, b * STEP]);
	};
	const resize = () => {
		dpr = Math.min(devicePixelRatio || 1, 2);
		w = canvas.clientWidth;
		h = canvas.clientHeight;
		canvas.width = w * dpr;
		canvas.height = h * dpr;
		ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
		ink = document.documentElement.dataset.theme === 'light' ? '0,0,0' : '255,255,255';
		pkts = Array.from({ length: Math.round((w * h) / 26000) }, () => ({
			pts: route(),
			t: Math.random(),
			speed: 0.0012 + Math.random() * 0.002,
		}));
	};
	const lengths = (p: Pkt) => {
		const segs = p.pts.slice(1).map((q, i) => Math.abs(q[0] - p.pts[i][0]) + Math.abs(q[1] - p.pts[i][1]));
		return { segs, total: segs.reduce((a, b) => a + b, 0) };
	};
	const at = (p: Pkt, d: number): [number, number] => {
		const { segs } = lengths(p);
		for (let i = 0; i < segs.length; i++) {
			if (d <= segs[i] || i === segs.length - 1) {
				const [x0, y0] = p.pts[i];
				const [x1, y1] = p.pts[i + 1];
				const f = segs[i] ? Math.min(d / segs[i], 1) : 0;
				return [x0 + (x1 - x0) * f, y0 + (y1 - y0) * f];
			}
			d -= segs[i];
		}
		return p.pts[0];
	};
	const draw = () => {
		ctx.clearRect(0, 0, w, h);
		for (const p of pkts) {
			const { total } = lengths(p);
			// Трасса — едва заметная линия.
			ctx.strokeStyle = `rgba(${ink},0.06)`;
			ctx.lineWidth = 1;
			ctx.beginPath();
			p.pts.forEach(([x, y], i) => (i ? ctx.lineTo(x + 0.5, y + 0.5) : ctx.moveTo(x + 0.5, y + 0.5)));
			ctx.stroke();
			// Пакет с хвостом.
			const head = p.t * total;
			for (let k = 0; k < 14; k++) {
				const d = head - k * 4;
				if (d < 0) break;
				const [x, y] = at(p, d);
				ctx.fillStyle = `rgba(${ink},${(0.55 * (1 - k / 14)).toFixed(3)})`;
				ctx.fillRect(x - 1, y - 1, k ? 2 : 3, k ? 2 : 3);
			}
			p.t += p.speed;
			if (p.t > 1) Object.assign(p, { pts: route(), t: 0 });
		}
	};
	let visible = true;
	let raf = 0;
	const loop = () => {
		draw();
		raf = visible ? requestAnimationFrame(loop) : 0;
	};
	resize();
	addEventListener('resize', resize);
	new MutationObserver(resize).observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
	if (reduced) return draw();
	new IntersectionObserver(([e]) => {
		visible = e.isIntersecting;
		if (visible && !raf) loop();
	}).observe(canvas);
	loop();
}

export function start() {
	reveal();
	copyButtons();
	theme();
	heroTerminal();
	anatomy();
	drift();
	trace();
	vars();
	field();
}
