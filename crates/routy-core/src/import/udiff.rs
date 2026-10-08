//! Unified diff по строкам для `--fix --dry-run` и предпросмотра правок в приложении.

/// Строк контекста вокруг изменений.
const CONTEXT: usize = 2;
/// Больше строк — показываем замену целиком, без LCS (O(n·m) памяти).
const MAX_LINES: usize = 4000;

/// Diff `before` → `after` с заголовками `--- a/<name>` / `+++ b/<name>`; пусто, если равны.
pub fn unified(name: &str, before: &str, after: &str) -> String {
    if before == after {
        return String::new();
    }
    let a: Vec<&str> = before.lines().collect();
    let b: Vec<&str> = after.lines().collect();
    let ops = ops(&a, &b);
    let mut out = format!("--- a/{name}\n+++ b/{name}\n");
    // Группы изменений с контекстом; соседние группы сливаются.
    let changed: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, o)| !matches!(o, Op::Same(..)))
        .map(|(i, _)| i)
        .collect();
    let mut i = 0;
    while i < changed.len() {
        let start = changed[i].saturating_sub(CONTEXT);
        let mut end = changed[i];
        while i + 1 < changed.len() && changed[i + 1] <= end + 2 * CONTEXT + 1 {
            i += 1;
            end = changed[i];
        }
        let end = (end + CONTEXT + 1).min(ops.len());
        let (mut a0, mut b0) = (0, 0);
        for o in &ops[..start] {
            match o {
                Op::Same(..) => (a0, b0) = (a0 + 1, b0 + 1),
                Op::Del(_) => a0 += 1,
                Op::Add(_) => b0 += 1,
            }
        }
        let hunk = &ops[start..end];
        let la = hunk.iter().filter(|o| !matches!(o, Op::Add(_))).count();
        let lb = hunk.iter().filter(|o| !matches!(o, Op::Del(_))).count();
        out.push_str(&format!(
            "@@ -{},{la} +{},{lb} @@\n",
            a0 + usize::from(la > 0),
            b0 + usize::from(lb > 0)
        ));
        for o in hunk {
            match o {
                Op::Same(l) => out.push_str(&format!(" {l}\n")),
                Op::Del(l) => out.push_str(&format!("-{l}\n")),
                Op::Add(l) => out.push_str(&format!("+{l}\n")),
            }
        }
        i += 1;
    }
    out
}

enum Op<'a> {
    Same(&'a str),
    Del(&'a str),
    Add(&'a str),
}

fn ops<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<Op<'a>> {
    // Общие начало и конец — без таблицы.
    let pre = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suf = a[pre..]
        .iter()
        .rev()
        .zip(b[pre..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (ma, mb) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    let mut out: Vec<Op> = a[..pre].iter().map(|l| Op::Same(l)).collect();
    if ma.len() > MAX_LINES || mb.len() > MAX_LINES {
        out.extend(ma.iter().map(|l| Op::Del(l)));
        out.extend(mb.iter().map(|l| Op::Add(l)));
    } else {
        // lcs[i][j] — длина LCS для ma[i..] и mb[j..].
        let mut lcs = vec![vec![0u32; mb.len() + 1]; ma.len() + 1];
        for i in (0..ma.len()).rev() {
            for j in (0..mb.len()).rev() {
                lcs[i][j] = if ma[i] == mb[j] {
                    lcs[i + 1][j + 1] + 1
                } else {
                    lcs[i + 1][j].max(lcs[i][j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < ma.len() || j < mb.len() {
            if i < ma.len() && j < mb.len() && ma[i] == mb[j] {
                out.push(Op::Same(ma[i]));
                i += 1;
                j += 1;
            } else if i < ma.len() && (j == mb.len() || lcs[i + 1][j] >= lcs[i][j + 1]) {
                out.push(Op::Del(ma[i]));
                i += 1;
            } else {
                out.push(Op::Add(mb[j]));
                j += 1;
            }
        }
    }
    out.extend(a[a.len() - suf..].iter().map(|l| Op::Same(l)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hunks() {
        assert_eq!(unified("x", "a\n", "a\n"), "");
        let before = "1\n2\n3\n4\n5\n6\n7\n8\n9\n";
        let after = "1\n2\n3\nfour\n5\n6\n7\n8\n9\nten\n";
        assert_eq!(
            unified("f.routy", before, after),
            "--- a/f.routy\n+++ b/f.routy\n@@ -2,5 +2,5 @@\n 2\n 3\n-4\n+four\n 5\n 6\n@@ -8,2 +8,3 @@\n 8\n 9\n+ten\n"
        );
    }
}
