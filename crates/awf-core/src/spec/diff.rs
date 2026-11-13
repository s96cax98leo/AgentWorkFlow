//! 逐行的統一格式差異（unified diff）。自己算，不呼叫外部程式；規格檔不大，用 LCS 就夠。

use super::read::split_lines;

const CONTEXT: usize = 3;
/// LCS 表格的格數上限；超過就把中間整段當成全換（結果仍然正確，只是不是最短）。
const MAX_CELLS: usize = 16_000_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Keep,
    Del,
    Add,
}

fn ops<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<(Op, &'a str)> {
    let pre = a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count();
    let (a_rest, b_rest) = (a.get(pre..).unwrap_or(&[]), b.get(pre..).unwrap_or(&[]));
    let suf = a_rest.iter().rev().zip(b_rest.iter().rev()).take_while(|(x, y)| x == y).count();
    let am = a_rest.get(..a_rest.len() - suf).unwrap_or(&[]);
    let bm = b_rest.get(..b_rest.len() - suf).unwrap_or(&[]);
    let mut out: Vec<(Op, &str)> = a.iter().take(pre).map(|l| (Op::Keep, *l)).collect();
    let (n, m) = (am.len(), bm.len());
    if n == 0 || m == 0 || n.saturating_mul(m) > MAX_CELLS {
        out.extend(am.iter().map(|l| (Op::Del, *l)));
        out.extend(bm.iter().map(|l| (Op::Add, *l)));
    } else {
        // t[i][j]＝am[i..] 與 bm[j..] 的 LCS 長度
        let w = m + 1;
        let mut t = vec![0u32; (n + 1) * w];
        let at = |t: &[u32], i: usize, j: usize| t.get(i * w + j).copied().unwrap_or(0);
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                let v = if am.get(i) == bm.get(j) { at(&t, i + 1, j + 1) + 1 } else { at(&t, i + 1, j).max(at(&t, i, j + 1)) };
                if let Some(cell) = t.get_mut(i * w + j) {
                    *cell = v;
                }
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n || j < m {
            match (am.get(i), bm.get(j)) {
                (Some(x), Some(y)) if x == y => {
                    out.push((Op::Keep, *x));
                    i += 1;
                    j += 1;
                }
                (Some(x), Some(_)) if at(&t, i + 1, j) >= at(&t, i, j + 1) => {
                    out.push((Op::Del, *x));
                    i += 1;
                }
                (_, Some(y)) => {
                    out.push((Op::Add, *y));
                    j += 1;
                }
                (Some(x), None) => {
                    out.push((Op::Del, *x));
                    i += 1;
                }
                (None, None) => break,
            }
        }
    }
    out.extend(a.iter().skip(a.len() - suf).map(|l| (Op::Keep, *l)));
    out
}

/// 兩份文字的 unified diff；一樣就回空字串。`old` 是 `None` 代表新檔（標頭用 `/dev/null`）。
pub fn unified(old: Option<&str>, new: &str, label: &str) -> String {
    let a = old.map(split_lines).unwrap_or_default();
    let b = split_lines(new);
    let ops = ops(&a, &b);
    let changed: Vec<usize> = ops.iter().enumerate().filter(|(_, o)| o.0 != Op::Keep).map(|(i, _)| i).collect();
    if changed.is_empty() {
        return String::new();
    }
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for &i in &changed {
        let (s, e) = (i.saturating_sub(CONTEXT), (i + CONTEXT + 1).min(ops.len()));
        match ranges.last_mut() {
            Some(last) if s <= last.1 => last.1 = e,
            _ => ranges.push((s, e)),
        }
    }
    let mut out = String::new();
    out.push_str(&match old {
        Some(_) => format!("--- a/{label}\n"),
        None => "--- /dev/null\n".to_string(),
    });
    out.push_str(&format!("+++ b/{label}\n"));
    let (mut ai, mut bi, mut pos) = (0usize, 0usize, 0usize);
    for (s, e) in ranges {
        for o in ops.get(pos..s).unwrap_or(&[]) {
            ai += usize::from(o.0 != Op::Add);
            bi += usize::from(o.0 != Op::Del);
        }
        let hunk = ops.get(s..e).unwrap_or(&[]);
        let al = hunk.iter().filter(|o| o.0 != Op::Add).count();
        let bl = hunk.iter().filter(|o| o.0 != Op::Del).count();
        let start = |at: usize, len: usize| if len == 0 { at } else { at + 1 };
        out.push_str(&format!("@@ -{},{al} +{},{bl} @@\n", start(ai, al), start(bi, bl)));
        for (op, line) in hunk {
            out.push(match op {
                Op::Keep => ' ',
                Op::Del => '-',
                Op::Add => '+',
            });
            out.push_str(line);
            out.push('\n');
        }
        ai += al;
        bi += bl;
        pos = e;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_text_gives_nothing() {
        assert_eq!(unified(Some("a\nb\n"), "a\r\nb\r\n", "f"), "");
    }

    #[test]
    fn replace_in_the_middle() {
        let old = "1\n2\n3\n4\n5\n6\n7\n8\n9\n";
        let new = "1\n2\n3\n4\n五\n6\n7\n8\n9\n";
        assert_eq!(unified(Some(old), new, "x/y.md"), "--- a/x/y.md\n+++ b/x/y.md\n@@ -2,7 +2,7 @@\n 2\n 3\n 4\n-5\n+五\n 6\n 7\n 8\n");
    }

    #[test]
    fn new_file_and_append() {
        assert_eq!(unified(None, "甲\n乙\n", "f"), "--- /dev/null\n+++ b/f\n@@ -0,0 +1,2 @@\n+甲\n+乙\n");
        assert_eq!(unified(Some("a\n"), "a\nb\n", "f"), "--- a/f\n+++ b/f\n@@ -1,1 +1,2 @@\n a\n+b\n");
        assert_eq!(unified(Some("a\nb\n"), "", "f"), "--- a/f\n+++ b/f\n@@ -1,2 +0,0 @@\n-a\n-b\n");
    }

    #[test]
    fn far_apart_changes_make_two_hunks() {
        let old: String = (1..=20).map(|i| format!("{i}\n")).collect();
        let new = old.replace("2\n3\n", "2\n三\n").replace("18\n", "十八\n");
        let d = unified(Some(&old), &new, "f");
        assert_eq!(d.matches("@@ -").count(), 2);
        assert!(d.contains("@@ -15,6 +15,6 @@"), "{d}");
    }
}
