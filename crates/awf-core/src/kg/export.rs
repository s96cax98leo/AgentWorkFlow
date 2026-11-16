//! `awf kg export`：把圖寫成 Obsidian 可以開的 vault（第 5 項才補齊）。

use super::build::Layout;
use super::query::Graph;

pub struct ExportReport {
    pub dir: String,
    pub pages: usize,
    pub kept: Vec<String>,
}

pub fn export(_g: &Graph, _lay: &Layout, _out: Option<&str>) -> Result<ExportReport, String> {
    Err("export 尚未實作".into())
}
