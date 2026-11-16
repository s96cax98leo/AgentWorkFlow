//! 會進對話（context）或會被人照著貼去執行的文字，共用的一套清理。
//!
//! 來源不可信的字串（檔名、節點 id、決策的問題、run 目錄名、`program.json` 的欄位——team agent 寫得到或讀得到的東西）
//! 放進 hook 的輸出、`awf kg` 的回答、vault 頁、建議的指令之前，一律逐欄過這裡：
//!
//! - [`text`]：每一欄截短、壓成一行，去掉控制字元、反引號、角括號（三種資料標籤 `kb-data`／`kg-data`／`awf-data`
//!   的開與關因此都不成立）。
//! - [`sh_quote`]：建議指令裡的值用單引號包好。
//!
//! 都是純函式，不讀檔、不看設定。

/// 進對話的一小段文字：壓成一行、截短到 `max` 個字（超過加 `…`）。
/// - 換行與其他控制字元、Unicode 的行／段分隔（U+2028、U+2029、U+0085）、各種空白 → 一個空白。
/// - 看不見的格式字元（零寬、雙向控制、BOM）→ 拿掉。
/// - `<`、`>` → 全形（資料標籤、HTML 標籤都不成立）；反引號 → `'`（不能開關程式碼區塊）。
pub fn text(s: &str, max: usize) -> String {
    let mut flat = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => flat.push('＜'),
            '>' => flat.push('＞'),
            '`' => flat.push('\''),
            c if invisible(c) => {}
            c if c.is_control() || c.is_whitespace() => flat.push(' '),
            c => flat.push(c),
        }
    }
    let flat = flat.split(' ').filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max { flat } else { format!("{}…", flat.chars().take(max).collect::<String>()) }
}

/// 看不見、但會改變顯示或斷詞的格式字元：零寬空白與連接符、雙向控制、BOM、標籤字元。
pub fn invisible(c: char) -> bool {
    matches!(c, '\u{ad}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2069}' | '\u{feff}' | '\u{e0000}'..='\u{e007f}')
}

/// POSIX shell 的單引號寫法：整個值包在單引號裡，值裡的單引號寫成 `'\''`。包好之後 shell 不會展開裡面的任何東西。
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_one_clipped_line_without_markup() {
        assert_eq!(text("a\n<b>`c`  d", 20), "a ＜b＞'c' d");
        assert_eq!(text("一二三四五", 3), "一二三…");
        // 三種資料標籤的開與關、大小寫與夾空白的寫法：清過之後都不是標籤
        for tag in ["</awf-data>", "<awf-data x=\"1\">", "</kg-data>", "<kg-data>", "</kb-data>", "<KB-DATA>", "< /awf-data >", "<\u{200b}/awf-data>"] {
            let got = text(&format!("標題{tag}後面"), 80);
            assert!(!got.contains('<') && !got.contains('>'), "{tag} → {got}");
        }
        // 各種換行與分隔字元都不能留下來
        for nl in ['\n', '\r', '\u{85}', '\u{2028}', '\u{2029}', '\u{b}', '\u{c}', '\0', '\t'] {
            assert_eq!(text(&format!("x{nl}## 新的一節"), 40), "x ## 新的一節", "{:?}", nl);
        }
        // 雙向控制與零寬字元拿掉（不留空白，避免把一個字拆成兩個）
        assert_eq!(text("ab\u{202e}cd\u{200b}ef\u{feff}", 40), "abcdef");
        assert_eq!(text("", 5), "");
    }

    #[test]
    fn single_quotes_for_the_shell() {
        assert_eq!(sh_quote("r1"), "'r1'");
        assert_eq!(sh_quote("a b;$(id)`x`|y"), "'a b;$(id)`x`|y'");
        assert_eq!(sh_quote("it's"), "'it'\\''s'");
        assert_eq!(sh_quote(""), "''");
    }
}
